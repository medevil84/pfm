//! Service-level gate tests for the deposit-interest engine. The four gates pin
//! the core invariants:
//!
//!  1. Mid-year deposit: day-by-day interest accounts for the mid-period
//!     balance change and differs from a naive "opening balance for the whole
//!     period" calculation.
//!  2. Withholding: the 26 % withholding is rounded half-even; when the exact
//!     product is x.5 the even cent is chosen (banker's rounding).
//!  3. Consolidation: the interest pair is written atomically; the net credit
//!     is verifiable in the linked current account's ledger.
//!  4. Idempotency: re-consolidating the same period replaces the prior pair
//!     (deterministic pair_id) rather than stacking a second credit.

use chrono::NaiveDate;
use pfm_repo::{DbSettings, Engine, Storage};
use pfm_service::DepositService;

const OWNER: &str = "u1";
const DEPOSIT: &str = "dep1";
const PRIMARY: &str = "cur1";

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

async fn setup() -> (DepositService, Storage) {
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

    // Seed owner.
    sqlx::query(
        "INSERT INTO users (id, username, email, password_hash, role, active, locale, default_currency, created_at)
         VALUES ('u1', 'owner', 'o@example.com', 'x', 'user', 1, 'en', 'USD', '2025-01-01T00:00:00Z')",
    )
    .execute(pool)
    .await
    .unwrap();

    // Primary (current) account — the backing account that receives net interest.
    sqlx::query(
        "INSERT INTO accounts (id, owner_id, name, kind, start_date, start_balance_cents, active, created_at)
         VALUES ('cur1', 'u1', 'corrente', 'current', '2023-01-01', 0, 1, '2025-01-01T00:00:00Z')",
    )
    .execute(pool)
    .await
    .unwrap();

    // Deposit account: 1 % annual rate (100 bps), linked to the current account.
    sqlx::query(
        "INSERT INTO accounts (id, owner_id, name, kind, start_date, start_balance_cents,
         primary_account_id, rate_pct_bps, active, created_at)
         VALUES ('dep1', 'u1', 'deposito', 'deposit', '2023-01-01', 0,
         'cur1', 100, 1, '2025-01-01T00:00:00Z')",
    )
    .execute(pool)
    .await
    .unwrap();

    let svc = DepositService::new(&storage);
    (svc, storage)
}

/// Insert a transaction directly into the ledger.
async fn seed_tx(pool: &sqlx::AnyPool, id: &str, account: &str, date: &str, cents: i64) {
    sqlx::query(
        "INSERT INTO transactions \
         (id, client_id, account_id, date, cents, description, created_by, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8)",
    )
    .bind(id)
    .bind(format!("seed-{id}"))
    .bind(account)
    .bind(date)
    .bind(cents)
    .bind("seed")
    .bind(OWNER)
    .bind("2023-01-01T00:00:00Z")
    .execute(pool)
    .await
    .unwrap();
}

/// Sum of all credits (positive cents) on an account over a date range.
async fn sum_credits(pool: &sqlx::AnyPool, account: &str, from: &str, to: &str) -> i64 {
    let row: (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(cents), 0) FROM transactions \
         WHERE account_id = $1 AND date >= $2 AND date <= $3 AND cents > 0",
    )
    .bind(account)
    .bind(from)
    .bind(to)
    .fetch_one(pool)
    .await
    .unwrap();
    row.0
}

/// Gate 1: a mid-year deposit changes the balance, so the day-by-day
/// computation (which sees the higher balance in the second half) must yield a
/// larger gross than the naive calculation that applies the opening balance to
/// the entire period.
#[tokio::test]
async fn mid_year_deposit_differs_from_simple_annual() {
    let (svc, storage) = setup().await;
    let pool = storage.pool();

    // 125.00 deposited on Jan 1, another 125.00 on Jul 1.
    seed_tx(pool, "d1", DEPOSIT, "2023-01-01", 12_500).await;
    seed_tx(pool, "d2", DEPOSIT, "2023-07-01", 12_500).await;

    // Preview the full year.
    let result = svc
        .preview_interest(OWNER, DEPOSIT, d("2023-01-01"), d("2023-12-31"))
        .await
        .unwrap();

    // Day-by-day:
    //   182 days (Jan 1 – Jun 30) at 12,500 → 12,500 × 100 × 182 = 227,500,000
    //   183 days (Jul 1 – Dec 31) at 25,000 → 25,000 × 100 × 183 = 457,500,000
    //   total_num = 685,000,000
    //   gross = round(685,000,000 / 3,650,000) = round(187.6712…) = 188
    assert_eq!(result.gross_cents, 188, "day-by-day gross");

    // Naive "simple annual" using only the opening balance (12,500) for all 365 days:
    //   12,500 × 100 × 365 / 3,650,000 = 125
    let naive = 125;
    assert_ne!(
        result.gross_cents, naive,
        "engine must account for the mid-period deposit"
    );
}

/// Gate 2: when the exact withholding product is x.5, round-half-even picks the
/// even cent. A gross of 125 gives 125 × 26 / 100 = 32.5 → 32 (even).
#[tokio::test]
async fn withholding_rounds_half_even() {
    let (svc, storage) = setup().await;
    let pool = storage.pool();

    // Constant 125.00 balance for exactly 365 days at 1 % (100 bps):
    //   total_num = 12,500 × 100 × 365 = 456,250,000
    //   gross = 456,250,000 / 3,650,000 = 125 (exact)
    //   withholding = round(125 × 26 / 100) = round(32.5) = 32 (even)
    //   net = 125 − 32 = 93
    seed_tx(pool, "d1", DEPOSIT, "2023-01-01", 12_500).await;

    let result = svc
        .preview_interest(OWNER, DEPOSIT, d("2023-01-01"), d("2023-12-31"))
        .await
        .unwrap();

    assert_eq!(result.gross_cents, 125);
    assert_eq!(
        result.withholding_cents, 32,
        "half-even rounds 32.5 down to 32"
    );
    assert_eq!(result.net_cents, 93);
    // Identity: gross = withholding + net.
    assert_eq!(
        result.gross_cents,
        result.withholding_cents + result.net_cents
    );
}

/// Gate 3: consolidating interest writes the pair atomically; the net credit
/// appears in the primary (current) account's ledger, and the gross credit
/// appears in the deposit account's ledger.
#[tokio::test]
async fn consolidation_credits_linked_account() {
    let (svc, storage) = setup().await;
    let pool = storage.pool();

    // 125.00 constant for 365 days at 1 % → gross 125, withholding 32, net 93.
    seed_tx(pool, "d1", DEPOSIT, "2023-01-01", 12_500).await;

    let outcome = svc
        .consolidate_interest(OWNER, DEPOSIT, d("2023-01-01"), d("2023-12-31"), false)
        .await
        .unwrap();

    assert_eq!(outcome.gross_cents, 125);
    assert_eq!(outcome.withholding_cents, 32);
    assert_eq!(outcome.net_cents, 93);
    assert!(
        outcome.pair_id.is_some(),
        "pair_id must be set on real consolidation"
    );

    // Verify the net credit landed in the primary account's register.
    let primary_credits = sum_credits(pool, PRIMARY, "2023-01-01", "2023-12-31").await;
    assert_eq!(primary_credits, 93, "net credit in primary account ledger");

    // Verify the gross credit landed in the deposit account's register.
    let deposit_credits = sum_credits(pool, DEPOSIT, "2023-01-01", "2023-12-31").await;
    // The deposit account has the seed (12,500) plus the gross interest (125).
    assert_eq!(deposit_credits, 12_500 + 125);
}

/// Gate 4: consolidating the same period twice is idempotent — the
/// deterministic pair_id causes the second call to delete and replace the
/// first pair, so the ledger shows exactly one net credit, not two.
#[tokio::test]
async fn re_consolidation_is_idempotent() {
    let (svc, storage) = setup().await;
    let pool = storage.pool();

    seed_tx(pool, "d1", DEPOSIT, "2023-01-01", 12_500).await;

    let first = svc
        .consolidate_interest(OWNER, DEPOSIT, d("2023-01-01"), d("2023-12-31"), false)
        .await
        .unwrap();
    let pid = first.pair_id.clone().unwrap();

    // Consolidate the same period again.
    let second = svc
        .consolidate_interest(OWNER, DEPOSIT, d("2023-01-01"), d("2023-12-31"), false)
        .await
        .unwrap();

    // Same pair_id (deterministic).
    assert_eq!(second.pair_id.as_deref(), Some(pid.as_str()));

    // The primary account has exactly one net credit (93), not two (186).
    let primary_credits = sum_credits(pool, PRIMARY, "2023-01-01", "2023-12-31").await;
    assert_eq!(
        primary_credits, 93,
        "re-consolidation must replace, not stack"
    );

    // Exactly one pair leg on each account.
    let leg_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM transactions WHERE pair_id = $1")
        .bind(&pid)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(leg_count.0, 2, "one leg per account");
}

/// Dry-run consolidation computes the interest but writes nothing to the ledger.
#[tokio::test]
async fn dry_run_does_not_write() {
    let (svc, storage) = setup().await;
    let pool = storage.pool();

    seed_tx(pool, "d1", DEPOSIT, "2023-01-01", 12_500).await;

    let outcome = svc
        .consolidate_interest(OWNER, DEPOSIT, d("2023-01-01"), d("2023-12-31"), true)
        .await
        .unwrap();

    assert_eq!(outcome.gross_cents, 125);
    assert_eq!(outcome.net_cents, 93);
    assert!(
        outcome.pair_id.is_none(),
        "dry-run must not produce a pair_id"
    );

    // No interest rows in either account.
    let primary_credits = sum_credits(pool, PRIMARY, "2023-01-01", "2023-12-31").await;
    assert_eq!(primary_credits, 0, "dry-run must not credit the primary");
}
