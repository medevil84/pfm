//! Repository-level atomicity tests for transfer pairs on SQLite in-memory.
//!
//! The schema comes from the **real migrations**, not from hand-written
//! `CREATE TABLE` statements. The previous version built its own schema and
//! omitted every index, which is why it never caught the `client_id` defect:
//! the UNIQUE index that the multi-row writers violate simply did not exist in
//! the tests. A suite that validates a different schema from the one the
//! application runs against cannot be used as a regression baseline.
//!
//! The old module comment blamed "a pre-existing UNIQUE-constraint bug on
//! `:memory:`" for avoiding `migrate()`. There is no such bug: migration 0003
//! seeds two fallback categories, one of them with `key = 'varie'`, and the
//! test below used to insert its own category with that same key against a
//! UNIQUE index on `categories.key`. The collision was the test's, not the
//! migrator's.
//!
//! `sqlite_apply_pragmas: true` ensures `PRAGMA foreign_keys=ON` is applied via
//! `after_connect` on the single pooled connection, so FK violations are raised
//! during the second INSERT of `create_transfer_pair`.

use chrono::NaiveDate;
use pfm_domain::{AccountRule, Cadence, CalcType, CondBase, FeeRow, Transaction};
use pfm_repo::{DbSettings, Engine, SqlxTransactionsRepo, Storage, TransactionsRepo};
use sqlx::Row;

async fn setup() -> (Storage, SqlxTransactionsRepo) {
    sqlx::any::install_default_drivers();

    let settings = DbSettings {
        engine: Engine::Sqlite,
        sqlite_path: Some(":memory:".to_string()),
        sqlite_apply_pragmas: true,
        sqlite_max_connections: 1,
        mariadb_url: None,
        mariadb_max_connections: 0,
        mariadb_min_connections: 0,
    };
    let storage = Storage::connect(&settings).await.expect("connect");
    let migrator = sqlx::migrate!("../../migrations");
    storage.migrate(&migrator).await.expect("migrate");
    let pool = storage.pool();

    sqlx::query(
        "INSERT INTO users (id, username, email, password_hash, created_at)
         VALUES ('u1', 'testuser', 't@e.com', '$argon2id$x', '2024-01-01T00:00:00Z')",
    )
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO accounts (id, owner_id, name, kind, start_date, created_at)
         VALUES ('acc-from', 'u1', 'From', 'current', '2024-01-01', '2024-01-01T00:00:00Z')",
    )
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO accounts (id, owner_id, name, kind, start_date, created_at)
         VALUES ('acc-to', 'u1', 'To', 'current', '2024-01-01', '2024-01-01T00:00:00Z')",
    )
    .execute(pool)
    .await
    .unwrap();

    let repo = SqlxTransactionsRepo::new(&storage);
    (storage, repo)
}

fn date() -> NaiveDate {
    NaiveDate::from_ymd_opt(2024, 6, 15).unwrap()
}

fn date_in(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

/// A successful `create_transfer_pair` followed by `delete_transfer_pair`
/// leaves zero rows with that `pair_id`.
#[tokio::test]
async fn delete_transfer_pair_removes_both_legs() {
    let (storage, repo) = setup().await;
    let pool = storage.pool();

    let pair_id = "pair-delete";
    let (from_leg, to_leg) = repo
        .create_transfer_pair(
            pair_id,
            "acc-from",
            "acc-to",
            date(),
            5000,
            &None,
            &None,
            "u1",
            None,
        )
        .await
        .expect("create pair");

    // Both legs share the same pair_id and have opposite signs.
    assert_eq!(from_leg.pair_id.as_deref(), Some(pair_id));
    assert_eq!(to_leg.pair_id.as_deref(), Some(pair_id));
    assert_eq!(from_leg.cents, -5000);
    assert_eq!(to_leg.cents, 5000);

    repo.delete_transfer_pair(pair_id)
        .await
        .expect("delete pair");

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions WHERE pair_id = $1")
        .bind(pair_id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "delete_transfer_pair left orphan rows");
}

/// When the second INSERT in `create_transfer_pair` fails (FK violation on a
/// ghost `to` account), the entire transaction is rolled back and no rows are
/// persisted — the `from` leg is not left behind as an orphan.
#[tokio::test]
async fn create_transfer_pair_second_insert_failure_leaves_no_orphan_leg() {
    let (storage, repo) = setup().await;
    let pool = storage.pool();

    let pair_id = "pair-rollback";
    // `acc-from` exists; "ghost-account" does not → FK violation on the 2nd INSERT.
    let result = repo
        .create_transfer_pair(
            pair_id,
            "acc-from",
            "ghost-account",
            date(),
            5000,
            &None,
            &None,
            "u1",
            None,
        )
        .await;

    assert!(result.is_err(), "expected FK violation error, got Ok");

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions WHERE pair_id = $1")
        .bind(pair_id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(
        count, 0,
        "rollback failed: orphan from-leg rows remain for pair_id={pair_id}"
    );

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(total, 0, "rollback failed: rows in transactions table");
}

/// A successful `create_transfer_pair` yields exactly two linked legs in the
/// database, one negative and one positive.
#[tokio::test]
async fn create_transfer_pair_success_yields_two_linked_legs() {
    let (storage, repo) = setup().await;
    let pool = storage.pool();

    let pair_id = "pair-success";
    let (from_leg, to_leg) = repo
        .create_transfer_pair(
            pair_id,
            "acc-from",
            "acc-to",
            date(),
            7500,
            &Some("desc".to_string()),
            &None,
            "u1",
            None,
        )
        .await
        .expect("create pair");

    assert_eq!(from_leg.pair_id.as_deref(), Some(pair_id));
    assert_eq!(to_leg.pair_id.as_deref(), Some(pair_id));
    assert_eq!(from_leg.account_id, "acc-from");
    assert_eq!(to_leg.account_id, "acc-to");
    assert_eq!(from_leg.cents, -7500);
    assert_eq!(to_leg.cents, 7500);
    assert_eq!(from_leg.description.as_deref(), Some("desc"));
    assert_eq!(to_leg.description.as_deref(), Some("desc"));

    // Verify via raw SQL that exactly 2 rows exist for this pair.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions WHERE pair_id = $1")
        .bind(pair_id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(count, 2, "expected exactly 2 legs, got {count}");

    // Verify get_transfer_legs returns both.
    let legs = repo.get_transfer_legs(pair_id).await.expect("get legs");
    assert_eq!(legs.len(), 2);
}

/// A minimal spending-rule handle (only the fields `regenerate_rule_rows`
/// touches — id, account, category, name — are meaningful here).
fn fee_rule(id: &str, account: &str, category: &str) -> AccountRule {
    AccountRule {
        id: id.into(),
        account_id: account.into(),
        name: "fee".into(),
        category_id: category.into(),
        calc_type: CalcType::Fixed,
        value: 0,
        is_annual: false,
        prorata_days: false,
        annual_cap_cents: None,
        cadence: Cadence::Monthly,
        charge_day: 1,
        charge_month: None,
        cond_base: CondBase::Always,
        cond_op: None,
        cond_threshold: None,
        active: true,
        created_at: String::new(),
        updated_at: String::new(),
    }
}

/// A fresh `client_id` per call: the column carries a UNIQUE index.
fn next_client_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    format!("test-client-{}", N.fetch_add(1, Ordering::Relaxed))
}

/// Build a transaction to persist via the repo. `computed` is `None` for manual
/// rows and `Some(true)` for engine rows; `rule_id` tags engine rows.
fn tx(account: &str, date: &str, cents: i64, rule_id: Option<&str>, computed: bool) -> Transaction {
    Transaction {
        id: String::new(),
        client_id: next_client_id(),
        account_id: account.into(),
        date: NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap(),
        cents,
        description: Some("x".into()),
        category_id: None,
        note: None,
        pair_id: None,
        rec_id: None,
        rule_id: rule_id.map(str::to_string),
        override_: None,
        computed: if computed { Some(true) } else { None },
        vacct_id: None,
        import_hash: None,
        created_by: "u1".into(),
        created_at: String::new(),
        updated_at: String::new(),
    }
}

/// `list_noncomputed` and `sum_noncomputed_cents_before` exclude rows the rules
/// engine produced (`computed = 1`), so a base-ledger view never reflects any
/// rule's generated rows.
#[tokio::test]
async fn noncomputed_queries_exclude_engine_rows() {
    let (_storage, repo) = setup().await;

    // Two manual rows and one engine row, interleaved by date.
    repo.create(&tx("acc-from", "2024-06-01", 1000, None, false))
        .await
        .unwrap();
    repo.create(&tx("acc-from", "2024-06-10", -200, None, false))
        .await
        .unwrap();
    repo.create(&tx("acc-from", "2024-06-05", -999, Some("rX"), true))
        .await
        .unwrap();

    // list_noncomputed: only the two manual rows come back, engine row excluded.
    let rows = repo
        .list_noncomputed("acc-from", date_in("2024-06-01"), date_in("2024-06-30"))
        .await
        .unwrap();
    let cents: Vec<i64> = rows.iter().map(|r| r.cents).collect();
    assert_eq!(
        cents,
        vec![1000, -200],
        "engine row leaked into the base ledger"
    );

    // sum_noncomputed_cents_before: the engine row (dated before the bound) is
    // excluded, so the sum is the manual rows only (1000 - 200 = 800). Had the
    // engine row been counted it would be -199.
    let sum = repo
        .sum_noncomputed_cents_before("acc-from", date_in("2024-06-20"))
        .await
        .unwrap();
    assert_eq!(
        sum, 800,
        "computed rows must be excluded from the base balance"
    );
}

/// `regenerate_rule_rows` replaces exactly the rule's in-horizon engine rows and
/// leaves everything else intact: manual rows, other rules' rows, and this
/// rule's rows outside the horizon all survive.
#[tokio::test]
async fn regenerate_replaces_only_in_horizon_rule_rows() {
    let (storage, repo) = setup().await;
    let pool = storage.pool();

    // A valid category for the engine rows this will create. `key` carries a
    // UNIQUE index and migration 0003 already seeds 'varie', so this uses a
    // key of its own.
    sqlx::query(
        "INSERT INTO categories (id, key, name, emoji, kind, is_fallback, created_at)
         VALUES ('cat-1', 'test-rule-cat', 'Varie', 'x', 'out', 0, '1970-01-01T00:00:00Z')",
    )
    .execute(pool)
    .await
    .unwrap();

    // Manual rows (untouchable), another rule's engine row, and this rule's old
    // rows: one inside the horizon, one outside it.
    repo.create(&tx("acc-from", "2024-06-01", 1000, None, false))
        .await
        .unwrap();
    repo.create(&tx("acc-from", "2024-06-10", -200, None, false))
        .await
        .unwrap();
    repo.create(&tx("acc-from", "2024-06-12", -777, Some("other"), true))
        .await
        .unwrap();
    repo.create(&tx("acc-from", "2024-06-05", -55, Some("rX"), true))
        .await
        .unwrap(); // in-horizon, replaced
    repo.create(&tx("acc-from", "2024-07-05", -66, Some("rX"), true))
        .await
        .unwrap(); // out-of-horizon, kept

    let rows = [
        FeeRow {
            date: date_in("2024-06-08"),
            amount_cents: -111,
        },
        FeeRow {
            date: date_in("2024-06-20"),
            amount_cents: -222,
        },
    ];
    repo.regenerate_rule_rows(
        &fee_rule("rX", "acc-from", "cat-1"),
        "u1",
        date_in("2024-06-01"),
        date_in("2024-06-30"),
        &rows,
    )
    .await
    .expect("regenerate");

    // This rule's in-horizon rows are exactly the two new ones.
    let rx_in_horizon: Vec<(String, i64)> = {
        let rs = sqlx::query(
            "SELECT date, cents FROM transactions \
             WHERE rule_id = 'rX' AND computed = 1 AND date >= '2024-06-01' AND date <= '2024-06-30' \
             ORDER BY date",
        )
        .fetch_all(pool)
        .await
        .unwrap();
        rs.iter()
            .map(|r| {
                (
                    r.try_get::<String, _>("date").unwrap(),
                    r.try_get::<i64, _>("cents").unwrap(),
                )
            })
            .collect()
    };
    assert_eq!(
        rx_in_horizon,
        vec![
            ("2024-06-08".to_string(), -111),
            ("2024-06-20".to_string(), -222)
        ],
        "in-horizon rows were not cleanly replaced"
    );

    // The out-of-horizon engine row for this rule survived.
    let out_horizon: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM transactions WHERE rule_id = 'rX' AND computed = 1 AND date = '2024-07-05'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        out_horizon, 1,
        "out-of-horizon engine row was wrongly deleted"
    );

    // Manual rows and the other rule's engine row are untouched.
    let survivors: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM transactions \
         WHERE (rule_id IS NULL AND (computed IS NULL OR computed = 0)) OR (rule_id = 'other' AND computed = 1)",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(survivors, 3, "manual or other-rule rows were disturbed");
}

/// `delete_rule_rows` removes only the given rule's engine rows, leaving manual
/// rows and other rules' rows in place.
#[tokio::test]
async fn delete_rule_rows_is_scoped_to_the_rule() {
    let (storage, repo) = setup().await;

    repo.create(&tx("acc-from", "2024-06-01", 1000, None, false))
        .await
        .unwrap();
    repo.create(&tx("acc-from", "2024-06-05", -55, Some("rX"), true))
        .await
        .unwrap();
    repo.create(&tx("acc-from", "2024-06-12", -777, Some("other"), true))
        .await
        .unwrap();

    let removed = repo.delete_rule_rows("rX", "acc-from").await.unwrap();
    assert_eq!(removed, 1, "expected exactly rX's engine row to be removed");

    // The manual row and the other rule's engine row remain.
    let survivors: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM transactions \
         WHERE (rule_id IS NULL AND (computed IS NULL OR computed = 0)) OR (rule_id = 'other' AND computed = 1)",
    )
    .fetch_one(storage.pool())
    .await
    .unwrap();
    assert_eq!(
        survivors, 2,
        "delete_rule_rows touched rows outside the rule"
    );
}

/// A successful `create_card_charge_pair` yields exactly two linked legs: the
/// backing (primary) account debited, the card credited, sharing one `pair_id`.
#[tokio::test]
async fn create_card_charge_pair_success_yields_two_linked_legs() {
    let (storage, repo) = setup().await;
    let pool = storage.pool();

    let pair_id = "cc-success";
    let (primary_leg, card_leg) = repo
        .create_card_charge_pair(
            pair_id,
            "acc-from",
            "acc-to",
            date(),
            date(),
            5000,
            5000,
            &Some("card settlement".to_string()),
            "u1",
        )
        .await
        .expect("create pair");

    // Backing account is debited (negative), the card credited (positive).
    assert_eq!(primary_leg.account_id, "acc-from");
    assert_eq!(card_leg.account_id, "acc-to");
    assert_eq!(primary_leg.cents, -5000);
    assert_eq!(card_leg.cents, 5000);
    assert_eq!(primary_leg.pair_id.as_deref(), Some(pair_id));
    assert_eq!(card_leg.pair_id.as_deref(), Some(pair_id));

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions WHERE pair_id = $1")
        .bind(pair_id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(count, 2, "expected exactly 2 legs, got {count}");
}

/// When the second INSERT in `create_card_charge_pair` fails (FK violation on a
/// ghost card account), the whole settlement rolls back: the primary (backing)
/// leg is not left behind as an orphan.
#[tokio::test]
async fn create_card_charge_pair_second_insert_failure_leaves_no_orphan_leg() {
    let (storage, repo) = setup().await;
    let pool = storage.pool();

    let pair_id = "cc-rollback";
    // `acc-from` (primary) exists; "ghost-card" does not → FK violation on the
    // card leg INSERT, which must roll back the backing leg too.
    let result = repo
        .create_card_charge_pair(
            pair_id,
            "acc-from",
            "ghost-card",
            date(),
            date(),
            5000,
            5000,
            &Some("card settlement".to_string()),
            "u1",
        )
        .await;

    assert!(result.is_err(), "expected FK violation error, got Ok");

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions WHERE pair_id = $1")
        .bind(pair_id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(
        count, 0,
        "rollback failed: orphan backing-leg rows remain for pair_id={pair_id}"
    );

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(total, 0, "rollback failed: rows in transactions table");
}

/// Re-running a settlement with the same deterministic `pair_id` replaces it
/// (delete-then-reinsert) instead of stacking a second charge: the pair still
/// holds exactly two legs.
#[tokio::test]
async fn create_card_charge_pair_is_idempotent_on_rerun() {
    let (storage, repo) = setup().await;
    let pool = storage.pool();

    let pair_id = "cc-idempotent";
    for _ in 0..2 {
        repo.create_card_charge_pair(
            pair_id,
            "acc-from",
            "acc-to",
            date(),
            date(),
            5000,
            5000,
            &Some("card settlement".to_string()),
            "u1",
        )
        .await
        .expect("create pair");
    }

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions WHERE pair_id = $1")
        .bind(pair_id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(
        count, 2,
        "re-settlement stacked a second charge instead of replacing it"
    );

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(total, 2, "re-settlement left extra legs behind");
}

// ─── Non-regression: `client_id` uniqueness on multi-row writes ──────────────
//
// Migration 0009 puts a UNIQUE index on `transactions.client_id`. Every writer
// that inserts more than one row in a single transaction therefore has to give
// each row its own value. When they all shared one (the account's `owner_id`),
// the first row went in, the second failed the constraint, and the surrounding
// transaction rolled the whole thing back — so transfers, card settlements,
// interest consolidation, rule generation and CSV import all returned HTTP 500
// and wrote nothing.
//
// These tests assert the **row count in the database**, not just that the call
// returned `Ok`, because the failure mode was a silent rollback.

/// The unique index is actually present. If this fails, the schema under test
/// has drifted from the migrations and none of the tests below prove anything.
#[tokio::test]
async fn client_id_unique_index_exists() {
    let (storage, _repo) = setup().await;
    let pool = storage.pool();

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master \
         WHERE type = 'index' AND name = 'idx_transactions_client_id'",
    )
    .fetch_one(pool)
    .await
    .unwrap();

    assert_eq!(
        count, 1,
        "idx_transactions_client_id is missing: the test schema is not the real one"
    );
}

/// A transfer persists both legs, each with its own `client_id`.
#[tokio::test]
async fn transfer_pair_persists_two_rows_with_distinct_client_ids() {
    let (storage, repo) = setup().await;
    let pool = storage.pool();

    repo.create_transfer_pair(
        "pair-cid",
        "acc-from",
        "acc-to",
        date(),
        2500,
        &Some("rent".to_string()),
        &None,
        "u1",
        None,
    )
    .await
    .expect("transfer must not violate the client_id index");

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions WHERE pair_id = $1")
        .bind("pair-cid")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(count, 2, "a transfer must leave exactly two legs");

    let distinct: i64 =
        sqlx::query_scalar("SELECT COUNT(DISTINCT client_id) FROM transactions WHERE pair_id = $1")
            .bind("pair-cid")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(distinct, 2, "the two legs share a client_id");
}

/// Interest consolidation persists both legs.
#[tokio::test]
async fn deposit_interest_pair_persists_two_rows() {
    let (storage, repo) = setup().await;
    let pool = storage.pool();

    repo.create_deposit_interest_pair(
        "dep-cid",
        "acc-from",
        "acc-to",
        date(),
        1200,
        888,
        &Some("interest".to_string()),
        "u1",
    )
    .await
    .expect("consolidation must not violate the client_id index");

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions WHERE pair_id = $1")
        .bind("dep-cid")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(count, 2, "consolidation must leave exactly two legs");
}

/// A 50-row import inserts 50 rows. One shared `client_id` used to make this
/// insert zero.
#[tokio::test]
async fn batch_import_persists_every_row() {
    let (storage, repo) = setup().await;
    let pool = storage.pool();

    let rows: Vec<pfm_repo::ImportRowData> = (0..50)
        .map(|i| pfm_repo::ImportRowData {
            date: date_in("2024-06-01"),
            cents: -(100 + i as i64),
            description: Some(format!("row {i}")),
            import_hash: format!("hash-{i}"),
        })
        .collect();

    let inserted = repo
        .batch_import("acc-from", &rows, "u1")
        .await
        .expect("import must not violate the client_id index");
    assert_eq!(inserted, 50);

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions WHERE account_id = $1")
        .bind("acc-from")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(count, 50, "the import batch was rolled back");

    let distinct: i64 = sqlx::query_scalar("SELECT COUNT(DISTINCT client_id) FROM transactions")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(distinct, 50, "imported rows share a client_id");
}

/// Multi-period rule generation persists one row per period.
#[tokio::test]
async fn rule_generation_persists_every_row() {
    let (storage, repo) = setup().await;
    let pool = storage.pool();

    sqlx::query(
        "INSERT INTO categories (id, key, name, emoji, kind, is_fallback, created_at)
         VALUES ('cat-2', 'test-gen-cat', 'Spese', 'x', 'out', 0, '1970-01-01T00:00:00Z')",
    )
    .execute(pool)
    .await
    .unwrap();

    let rows = vec![
        FeeRow {
            date: date_in("2024-06-01"),
            amount_cents: -200,
        },
        FeeRow {
            date: date_in("2024-07-01"),
            amount_cents: -200,
        },
        FeeRow {
            date: date_in("2024-08-01"),
            amount_cents: -200,
        },
    ];

    repo.regenerate_rule_rows(
        &fee_rule("rGen", "acc-from", "cat-2"),
        "u1",
        date_in("2024-06-01"),
        date_in("2024-08-31"),
        &rows,
    )
    .await
    .expect("rule generation must not violate the client_id index");

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions WHERE rule_id = $1")
        .bind("rGen")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(count, 3, "rule generation was rolled back");
}
