//! Service-level gate tests for spending rules. The four gates pin the core
//! invariants of the rule ledger:
//!
//!  1. Regeneration never touches manual (`computed` null/`0`) rows.
//!  2. A `pct_avg_balance` rule is stable across regeneration: engine rows are
//!     excluded from the metrics, so the rule never sees its own output.
//!  3. The calendar-year `annual_cap_cents` is respected and stays respected
//!     across repeated regeneration (it never accumulates a second time).
//!  4. Two active rules on the same account are order-independent: each rule's
//!     output excludes the other rule's generated rows.

use chrono::NaiveDate;
use pfm_domain::{Cadence, CalcType, CondBase, CondOp, FeeRow, Transaction};
use pfm_repo::{DbSettings, Engine, SqlxTransactionsRepo, Storage, TransactionsRepo};
use pfm_service::{FeeRuleService, RuleCreateCmd, RuleRangeCmd};

const OWNER: &str = "u1";
const ACCOUNT: &str = "acc1";
const CAT_OUT: &str = "cat-fallback-out";

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

async fn setup() -> (FeeRuleService, Storage) {
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

    // Seed owner, an owned account (opened 2024-01-01 at zero), and the
    // fallback-out category the generated fee rows are classified under.
    sqlx::query(
        "INSERT INTO users (id, username, email, password_hash, role, active, locale, default_currency, created_at)
         VALUES ('u1', 'owner', 'o@example.com', 'x', 'user', 1, 'en', 'USD', '2025-01-01T00:00:00Z')",
    )
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO accounts (id, owner_id, name, kind, start_date, start_balance_cents, active, created_at)
         VALUES ('acc1', 'u1', 'conto', 'current', '2024-01-01', 0, 1, '2025-01-01T00:00:00Z')",
    )
    .execute(pool)
    .await
    .unwrap();

    let svc = FeeRuleService::new(&storage);
    (svc, storage)
}

/// Build a rule-creation command with the common defaults (fallback-out
/// category, always-true condition, non-annual).
fn make_rule(
    name: &str,
    calc: CalcType,
    value: i64,
    cadence: Cadence,
    charge_day: u32,
    cap: Option<i64>,
) -> RuleCreateCmd {
    RuleCreateCmd {
        name: name.into(),
        category_id: CAT_OUT.into(),
        calc_type: calc,
        value,
        is_annual: false,
        prorata_days: false,
        annual_cap_cents: cap,
        cadence,
        charge_day,
        charge_month: None,
        cond_base: CondBase::Always,
        cond_op: None,
        cond_threshold: None,
        active: true,
    }
}

fn range(from: &str, to: &str) -> RuleRangeCmd {
    RuleRangeCmd {
        from: Some(d(from)),
        to: Some(d(to)),
    }
}

/// Insert a manual (non-computed) transaction directly into the ledger.
async fn seed_tx(pool: &sqlx::AnyPool, id: &str, date: &str, cents: i64) {
    sqlx::query(
        "INSERT INTO transactions \
         (id, client_id, account_id, date, cents, description, created_by, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8)",
    )
    .bind(id)
    .bind(id)
    .bind(ACCOUNT)
    .bind(date)
    .bind(cents)
    .bind("seed")
    .bind(OWNER)
    .bind("2024-01-01T00:00:00Z")
    .execute(pool)
    .await
    .unwrap();
}

/// The (date, cents) of a sorted row list, used to prove the manual ledger is
/// byte-for-byte invariant.
fn keys(rows: &[Transaction]) -> Vec<(NaiveDate, i64)> {
    rows.iter().map(|r| (r.date, r.cents)).collect()
}

fn mags(rows: &[FeeRow]) -> Vec<i64> {
    rows.iter().map(|r| -r.amount_cents).collect()
}

/// Gate 1: generating and re-generating a rule only adds/replaces that rule's
/// `computed = 1` rows; the manual ledger is never read into or modified.
#[tokio::test]
async fn regeneration_never_touches_manual_rows() {
    let (svc, storage) = setup().await;
    let pool = storage.pool();

    seed_tx(pool, "m1", "2024-01-01", 5_000_000).await;
    seed_tx(pool, "m2", "2024-02-15", -100_000).await;

    let tx = SqlxTransactionsRepo::new(&storage);
    let manual_before = tx
        .list_noncomputed(ACCOUNT, d("2024-01-01"), d("2024-03-31"))
        .await
        .unwrap();
    assert_eq!(
        manual_before.len(),
        2,
        "seeded manual rows: {manual_before:?}"
    );

    let rule = svc
        .create(
            OWNER,
            ACCOUNT,
            &make_rule("fix", CalcType::Fixed, 50_000, Cadence::Monthly, 1, None),
        )
        .await
        .unwrap();

    let rows = svc
        .generate(OWNER, ACCOUNT, &rule.id, &range("2024-01-01", "2024-03-31"))
        .await
        .unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|r| r.amount_cents == -50_000));

    // Generation added engine rows but left the manual ledger exactly as it was.
    let after_gen = tx
        .list_noncomputed(ACCOUNT, d("2024-01-01"), d("2024-03-31"))
        .await
        .unwrap();
    assert_eq!(keys(&manual_before), keys(&after_gen));

    // Regenerate over the same horizon: the manual ledger is still untouched.
    let rows2 = svc
        .generate(OWNER, ACCOUNT, &rule.id, &range("2024-01-01", "2024-03-31"))
        .await
        .unwrap();
    assert_eq!(rows2.len(), 3);

    let after_regen = tx
        .list_noncomputed(ACCOUNT, d("2024-01-01"), d("2024-03-31"))
        .await
        .unwrap();
    assert_eq!(keys(&manual_before), keys(&after_regen));
}

/// Gate 2: a percentage-of-average-balance rule is idempotent. The engine rows
/// it writes are excluded from the metrics it feeds, so a re-generation is
/// bit-for-bit identical (the rule never compounds on its own output).
#[tokio::test]
async fn pct_avg_rule_is_stable_across_regeneration() {
    let (svc, storage) = setup().await;
    let pool = storage.pool();

    // A single deposit leaves the balance constant at 50,000.00 for the year.
    seed_tx(pool, "dep", "2024-01-01", 5_000_000).await;

    // 1% (100 bps) of a constant 50,000.00 balance = 500.00 (50,000 cents) monthly.
    let rule = svc
        .create(
            OWNER,
            ACCOUNT,
            &make_rule(
                "pct",
                CalcType::PctAvgBalance,
                100,
                Cadence::Monthly,
                1,
                None,
            ),
        )
        .await
        .unwrap();

    let first = svc
        .generate(OWNER, ACCOUNT, &rule.id, &range("2024-01-01", "2024-12-31"))
        .await
        .unwrap();
    assert_eq!(first.len(), 12);
    assert!(
        first.iter().all(|r| r.amount_cents == -50_000),
        "unexpected first pass: {first:?}"
    );

    // Re-generate: with the engine rows correctly excluded from the average
    // balance, the result is identical to the first pass.
    let second = svc
        .generate(OWNER, ACCOUNT, &rule.id, &range("2024-01-01", "2024-12-31"))
        .await
        .unwrap();
    assert_eq!(
        first, second,
        "self-referencing rule drifted on regeneration"
    );
}

/// Gate 2 (condition form): a rule whose condition is evaluated against the
/// average balance (`cond_base = 'avg_balance'`) must not see its own generated
/// rows when recomputing that balance. A fixed fee charged only while the
/// average balance stays above a threshold is a sensitive probe: the base
/// ledger keeps the balance just above the threshold, but the fee's own
/// outflow would pull it below — so any leak of the rule's rows back into the
/// balance would flip the condition and drift the result across regenerations.
#[tokio::test]
async fn avg_balance_condition_rule_has_no_self_reference() {
    let (svc, storage) = setup().await;
    let pool = storage.pool();

    // A single deposit leaves the balance constant at 50,000.00 for the year.
    seed_tx(pool, "dep", "2024-01-01", 5_000_000).await;

    // Fixed 1,000.00 fee charged each month only while the average balance is
    // >= 49,500.00. The engine-free average is 50,000.00, just above the
    // threshold; the fee's own 1,000.00 outflow would push a month below it, so
    // this catches any self-reference in the balance the condition reads.
    let cmd = RuleCreateCmd {
        name: "threshold".into(),
        category_id: CAT_OUT.into(),
        calc_type: CalcType::Fixed,
        value: 100_000,
        is_annual: false,
        prorata_days: false,
        annual_cap_cents: None,
        cadence: Cadence::Monthly,
        charge_day: 1,
        charge_month: None,
        cond_base: CondBase::AvgBalance,
        cond_op: Some(CondOp::Ge),
        cond_threshold: Some(4_950_000),
        active: true,
    };
    let rule = svc.create(OWNER, ACCOUNT, &cmd).await.unwrap();

    // Every month's average balance (50,000.00) clears the threshold: all 12
    // months are charged.
    let first = svc
        .generate(OWNER, ACCOUNT, &rule.id, &range("2024-01-01", "2024-12-31"))
        .await
        .unwrap();
    assert_eq!(first.len(), 12, "expected all 12 months charged: {first:?}");
    assert!(first.iter().all(|r| r.amount_cents == -100_000));

    // Several consecutive regenerations must be bit-for-bit identical: the
    // rule's own rows never feed back into the average balance it is gated on.
    for _ in 0..4 {
        let again = svc
            .generate(OWNER, ACCOUNT, &rule.id, &range("2024-01-01", "2024-12-31"))
            .await
            .unwrap();
        assert_eq!(
            first, again,
            "avg_balance condition self-referenced its own rows"
        );
    }
}

/// Gate 3: the calendar-year cap truncates the cumulative charge, and repeated
/// regeneration keeps the total pinned at the cap (it is never charged twice).
#[tokio::test]
async fn annual_cap_is_respected_and_stable() {
    let (svc, storage) = setup().await;
    let pool = storage.pool();

    seed_tx(pool, "dep", "2024-01-01", 5_000_000).await;

    // 1% of 50,000.00 = 500.00/mo, capped at 1,200.00 for the calendar year.
    let rule = svc
        .create(
            OWNER,
            ACCOUNT,
            &make_rule(
                "capped",
                CalcType::PctAvgBalance,
                100,
                Cadence::Monthly,
                1,
                Some(120_000),
            ),
        )
        .await
        .unwrap();

    let rows = svc
        .generate(OWNER, ACCOUNT, &rule.id, &range("2024-01-01", "2024-12-31"))
        .await
        .unwrap();
    let m = mags(&rows);
    // Jan 500, Feb 500, Mar 200 (reaches the 1,200 cap), the rest truncated.
    assert_eq!(m, vec![50_000, 50_000, 20_000]);
    assert_eq!(m.iter().sum::<i64>(), 120_000);

    // Stable across SEVERAL consecutive regenerations: the cap is recomputed from
    // the base ledger each pass, so it is never charged a second time, no matter
    // how many times the rule is regenerated within the same calendar year.
    for _ in 0..5 {
        let again = svc
            .generate(OWNER, ACCOUNT, &rule.id, &range("2024-01-01", "2024-12-31"))
            .await
            .unwrap();
        assert_eq!(rows, again, "annual cap accumulated across a re-generation");
        assert_eq!(mags(&again).iter().sum::<i64>(), 120_000);
    }
}

/// Gate 4: two active rules on the same account are order-independent. Each
/// rule's metrics exclude every engine row (including the other rule's), so the
/// result for a rule is the same whether the other rule's rows are already in
/// the ledger or not.
#[tokio::test]
async fn two_rules_are_order_independent() {
    let a_cmd = make_rule(
        "ruleA",
        CalcType::PctAvgBalance,
        100,
        Cadence::Monthly,
        1,
        None,
    );
    let b_cmd = make_rule(
        "ruleB",
        CalcType::PctAvgBalance,
        200,
        Cadence::Monthly,
        1,
        None,
    );

    // DB1: A generated on an empty ledger, then B (with A's rows present).
    let (s1, st1) = setup().await;
    seed_tx(st1.pool(), "dep", "2024-01-01", 5_000_000).await;
    let a1 = s1.create(OWNER, ACCOUNT, &a_cmd).await.unwrap();
    let b1 = s1.create(OWNER, ACCOUNT, &b_cmd).await.unwrap();
    let a_empty = s1
        .generate(OWNER, ACCOUNT, &a1.id, &range("2024-01-01", "2024-03-31"))
        .await
        .unwrap();
    let b_after_a = s1
        .generate(OWNER, ACCOUNT, &b1.id, &range("2024-01-01", "2024-03-31"))
        .await
        .unwrap();

    // DB2: B generated on an empty ledger, then A (with B's rows present).
    let (s2, st2) = setup().await;
    seed_tx(st2.pool(), "dep", "2024-01-01", 5_000_000).await;
    let a2 = s2.create(OWNER, ACCOUNT, &a_cmd).await.unwrap();
    let b2 = s2.create(OWNER, ACCOUNT, &b_cmd).await.unwrap();
    let b_empty = s2
        .generate(OWNER, ACCOUNT, &b2.id, &range("2024-01-01", "2024-03-31"))
        .await
        .unwrap();
    let a_after_b = s2
        .generate(OWNER, ACCOUNT, &a2.id, &range("2024-01-01", "2024-03-31"))
        .await
        .unwrap();

    // A computed on an empty ledger equals A computed with B's rows present
    // (and excluded). Likewise for B relative to A.
    assert_eq!(
        a_empty, a_after_b,
        "rule A depends on rule B's rows being present"
    );
    assert_eq!(
        b_empty, b_after_a,
        "rule B depends on rule A's rows being present"
    );

    // Sanity: A is 1% and B is 2% of the constant 50,000.00 balance, each month.
    assert_eq!(a_empty.len(), 3);
    assert!(a_empty.iter().all(|r| r.amount_cents == -50_000));
    assert!(b_empty.iter().all(|r| r.amount_cents == -100_000));
}
