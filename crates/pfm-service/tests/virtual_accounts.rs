//! Service-level gate tests for the virtual-account (piggy-bank) engine. The
//! three gates pin the core invariants:
//!
//!  1. Deselect recompute: folding a piggy bank into the selection reduces the
//!     "available" balance by exactly the amount set aside; deselecting it
//!     recovers that amount, exactly as if the piggy bank had never existed.
//!  2. Piggy self-view: a piggy bank's own set-aside legs are always part of
//!     its balance (its `vacct_id` equals its own id), even when the piggy is
//!     deselected everywhere.
//!  3. No double count: the money set aside is a re-slicing of the real
//!     account's physical balance. Net worth sums only real accounts' physical
//!     balances; the piggy totals are already contained in them, so they must
//!     never be added on top.

use pfm_repo::{DbSettings, Engine, RegisterFilters, Storage};
use pfm_service::{TransactionService, VirtualAccountService, VirtualError};

const OWNER: &str = "u1";
const OTHER: &str = "u2";
const REAL: &str = "real1";
const PIG1: &str = "pig1";
const PIG2: &str = "pig2";

/// Create an in-memory SQLite store with the tables the service touches and
/// seed the owner, a second (unrelated) user, the real account and two piggy
/// banks pointing at it.
async fn setup() -> (VirtualAccountService, Storage) {
    sqlx::any::install_default_drivers();

    let storage = Storage::connect(&DbSettings {
        engine: Engine::Sqlite,
        sqlite_path: Some(":memory:".into()),
        sqlite_apply_pragmas: false,
        sqlite_max_connections: 1,
        mariadb_url: None,
        mariadb_max_connections: 0,
        mariadb_min_connections: 0,
    })
    .await
    .unwrap();

    // Real schema, from the migrations the application actually runs — indexes
    // included. The hand-written DDL this replaces defined no indexes at all,
    // so constraint violations that break production went unnoticed here.
    let migrator = sqlx::migrate!("../../migrations");
    storage.migrate(&migrator).await.expect("migrations");

    let pool = storage.pool();

    // Prefs are read by the register (for the show-future flag); an empty table
    // yields default prefs.

    // Users: owner and an unrelated second user (for the forbidden gate).
    sqlx::query(
        "INSERT INTO users (id, username, email, password_hash, role, active, locale, default_currency, created_at)
         VALUES ('u1', 'owner', 'o@example.com', 'x', 'user', 1, 'en', 'USD', '2025-01-01T00:00:00Z'),
                ('u2', 'other', 't@example.com', 'x', 'user', 1, 'en', 'USD', '2025-01-01T00:00:00Z')",
    )
    .execute(pool)
    .await
    .unwrap();

    // Real (current) account: opens at 1,000.00.
    sqlx::query(
        "INSERT INTO accounts (id, owner_id, name, kind, start_date, start_balance_cents, active, created_at)
         VALUES ('real1', 'u1', 'corrente', 'current', '2023-01-01', 100000, 1, '2025-01-01T00:00:00Z')",
    )
    .execute(pool)
    .await
    .unwrap();

    // Two piggy banks, both slices of the real account, opening at 0.
    sqlx::query(
        "INSERT INTO accounts (id, owner_id, name, kind, start_date, start_balance_cents,
         primary_account_id, active, created_at)
         VALUES ('pig1', 'u1', 'tasse', 'virtual', '2023-01-01', 0, 'real1', 1, '2025-01-01T00:00:00Z'),
                ('pig2', 'u1', 'vacanze', 'virtual', '2023-01-01', 0, 'real1', 1, '2025-01-01T00:00:00Z')",
    )
    .execute(pool)
    .await
    .unwrap();

    let svc = VirtualAccountService::new(&storage);
    (svc, storage)
}

/// Insert a movement directly into the ledger, optionally tagged with a
/// piggy-bank id (`vacct`).
async fn seed_tx(
    pool: &sqlx::AnyPool,
    id: &str,
    account: &str,
    date: &str,
    cents: i64,
    vacct: Option<&str>,
) {
    sqlx::query(
        "INSERT INTO transactions \
         (id, client_id, account_id, date, cents, description, vacct_id, created_by, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $9)",
    )
    .bind(id)
    .bind(format!("seed-{id}"))
    .bind(account)
    .bind(date)
    .bind(cents)
    .bind("seed")
    .bind(vacct)
    .bind(OWNER)
    .bind("2023-01-01T00:00:00Z")
    .execute(pool)
    .await
    .unwrap();
}

/// Seed the shared scenario: a 50.00 expense on the real account (no
/// piggy), and set-asides of 100.00 and 200.00 from the real account to the
/// two piggy banks. Both legs of each set-aside carry the piggy's id.
async fn seed_scenario(pool: &sqlx::AnyPool) {
    // Ordinary (non-allocated) expense on the real account.
    seed_tx(pool, "e1", REAL, "2023-02-01", -5_000, None).await;

    // Set-aside of 100.00 real -> pig1: both legs tagged with pig1.
    seed_tx(pool, "s1a", REAL, "2023-03-01", -10_000, Some(PIG1)).await;
    seed_tx(pool, "s1b", PIG1, "2023-03-01", 10_000, Some(PIG1)).await;

    // Set-aside of 200.00 real -> pig2: both legs tagged with pig2.
    seed_tx(pool, "s2a", REAL, "2023-03-02", -20_000, Some(PIG2)).await;
    seed_tx(pool, "s2b", PIG2, "2023-03-02", 20_000, Some(PIG2)).await;
}

/// Gate 1: the "available" balance folds back exactly the selected piggy
/// banks' set-asides; the physical (bank) balance is unchanged by any
/// piggy selection, and deselecting a piggy recovers its set-aside.
#[tokio::test]
async fn deselect_recomputes_available_balance() {
    let (svc, storage) = setup().await;
    let pool = storage.pool();
    seed_scenario(pool).await;

    let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();

    // No piggy selected: available == physical (1,000.00 - 50.00 = 950.00).
    let none = svc.balance(OWNER, REAL, &[]).await.unwrap();
    assert_eq!(
        none.real_cents, 95_000,
        "physical balance ignores set-asides"
    );
    assert_eq!(
        none.allocated_cents, 95_000,
        "empty selection collapses to physical"
    );

    // One piggy selected: 100.00 set aside is recovered into "not available".
    let p1 = svc.balance(OWNER, REAL, &ids(&[PIG1])).await.unwrap();
    assert_eq!(
        p1.real_cents, 95_000,
        "physical balance unaffected by selection"
    );
    assert_eq!(
        p1.allocated_cents, 85_000,
        "available drops by the 100.00 set-aside"
    );

    // Both piggies selected: 300.00 total is set aside.
    let both = svc.balance(OWNER, REAL, &ids(&[PIG1, PIG2])).await.unwrap();
    assert_eq!(both.real_cents, 95_000);
    assert_eq!(
        both.allocated_cents, 65_000,
        "available drops by the 300.00 total"
    );

    // Deselecting both recovers exactly what was set aside.
    assert_eq!(
        none.allocated_cents - both.allocated_cents,
        30_000,
        "deselecting recovers the set-aside total"
    );
}

/// Gate 2: a piggy bank's own set-aside legs always belong to it, even when the
/// piggy is deselected everywhere (empty selection) — its register/balance is
/// never hollowed out by the owner's piggy toggles.
#[tokio::test]
async fn piggy_own_register_always_complete() {
    let (svc, storage) = setup().await;
    let pool = storage.pool();
    seed_scenario(pool).await;

    // Empty selection: pig1 still shows the 100.00 that was set aside into it.
    let pig1_empty = svc.balance(OWNER, PIG1, &[]).await.unwrap();
    assert_eq!(
        pig1_empty.real_cents, 0,
        "piggy has no non-allocated movements"
    );
    assert_eq!(
        pig1_empty.allocated_cents, 10_000,
        "own legs always visible"
    );

    // Selecting an *other* piggy does not change pig1's self-view.
    let other = vec![PIG2.to_string()];
    let pig1_other = svc.balance(OWNER, PIG1, &other).await.unwrap();
    assert_eq!(pig1_other.allocated_cents, pig1_empty.allocated_cents);

    // pig2 shows its full 200.00 as well.
    let pig2_empty = svc.balance(OWNER, PIG2, &[]).await.unwrap();
    assert_eq!(pig2_empty.allocated_cents, 20_000);
}

/// Gate 3: the money set aside is re-sliced out of the real account's physical
/// balance, not money that lives only in the piggy banks. Therefore net worth
/// (sum of real accounts' physical balances) must not add the piggy totals on
/// top — that would count the set-aside twice.
#[tokio::test]
async fn net_worth_does_not_double_count_set_asides() {
    let (svc, storage) = setup().await;
    let pool = storage.pool();
    seed_scenario(pool).await;

    let none = svc.balance(OWNER, REAL, &[]).await.unwrap();
    let both = svc
        .balance(OWNER, REAL, &[PIG1.to_string(), PIG2.to_string()])
        .await
        .unwrap();
    let pig1 = svc.balance(OWNER, PIG1, &[]).await.unwrap();
    let pig2 = svc.balance(OWNER, PIG2, &[]).await.unwrap();

    // The set-asides (300.00) are carved out of the physical balance:
    //   physical (950.00) = available-with-both (650.00) + set-aside (300.00).
    assert_eq!(
        none.real_cents,
        both.allocated_cents + pig1.allocated_cents + pig2.allocated_cents,
        "set-asides are a slice of the physical balance, not extra money"
    );

    // Net worth counts only the real account's physical balance. The piggy
    // balances (100.00 + 200.00) are already inside it, so they must NOT be
    // added on top (that would yield 1,250.00 instead of 950.00).
    let net_worth = none.real_cents;
    assert_eq!(net_worth, 95_000, "net worth = real physical balance only");
    assert_ne!(
        net_worth,
        net_worth + pig1.allocated_cents + pig2.allocated_cents,
        "summing piggies on top would double-count the set-asides"
    );
}

/// A balance request for an unknown account is a clean 404, not a 500.
#[tokio::test]
async fn unknown_account_is_not_found() {
    let (svc, storage) = setup().await;
    let pool = storage.pool();
    seed_scenario(pool).await;

    let err = svc.balance(OWNER, "does-not-exist", &[]).await.unwrap_err();
    assert!(
        matches!(err, VirtualError::AccountNotFound),
        "missing account maps to AccountNotFound, got {err:?}"
    );
}

/// A user with no ownership or share of the account is denied.
#[tokio::test]
async fn non_owner_without_share_is_forbidden() {
    let (svc, storage) = setup().await;
    let pool = storage.pool();
    seed_scenario(pool).await;

    let err = svc.balance(OTHER, REAL, &[]).await.unwrap_err();
    assert!(
        matches!(err, VirtualError::Forbidden),
        "unrelated user is denied, got {err:?}"
    );
}

/// Gate 1 (register view): on the real account, deselecting a piggy bank removes
/// its set-aside legs from the register and recomputes the running/closing
/// balance as if the piggy bank had never existed; selecting a piggy folds its
/// legs back in. This exercises the register's filtered fetch
/// (`list_for_register` + `sum_cents_before_filtered`), distinct from the
/// two-balance view.
#[tokio::test]
async fn register_hides_deselected_piggies_and_recomputes() {
    let (_svc, storage) = setup().await;
    let pool = storage.pool();
    seed_scenario(pool).await;

    let ts = TransactionService::new(&storage);
    let from = chrono::NaiveDate::parse_from_str("2023-01-01", "%Y-%m-%d").unwrap();
    let to = chrono::NaiveDate::parse_from_str("2023-12-31", "%Y-%m-%d").unwrap();

    fn filters(v: Vec<String>) -> RegisterFilters {
        RegisterFilters {
            category_id: None,
            tx_type: None,
            min_cents: None,
            max_cents: None,
            text: None,
            vacct_included: Some(v),
        }
    }

    // No piggy selected: only the ordinary expense is visible (set-asides hidden).
    let none = ts
        .register(OWNER, REAL, from, to, &filters(vec![]))
        .await
        .unwrap();
    let none_rows: usize = none.days.iter().map(|d| d.rows.len()).sum();
    assert_eq!(
        none_rows, 1,
        "only the expense is visible with no piggy selected"
    );
    assert_eq!(none.summary.opening_cents, 100_000);
    assert_eq!(
        none.summary.closing_cents, 95_000,
        "closing reflects only the expense"
    );

    // Both piggies selected: all three legs are shown and closing drops further.
    let both = ts
        .register(
            OWNER,
            REAL,
            from,
            to,
            &filters(vec![PIG1.to_string(), PIG2.to_string()]),
        )
        .await
        .unwrap();
    let both_rows: usize = both.days.iter().map(|d| d.rows.len()).sum();
    assert_eq!(both_rows, 3, "expense + both set-asides are visible");
    assert_eq!(both.summary.opening_cents, 100_000);
    assert_eq!(
        both.summary.closing_cents, 65_000,
        "closing folds both set-asides back in"
    );

    // Deselecting recovers exactly the set-aside total in the closing balance.
    assert_eq!(
        none.summary.closing_cents - both.summary.closing_cents,
        30_000,
        "deselecting the piggies recovers their set-asides"
    );
}
