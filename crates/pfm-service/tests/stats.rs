//! Service-level gate tests for the stats feature:
//!
//! 1. **Projection coherence** — the future (forecast) part of the balance
//!    trend produced by `StatsService::stats` must match what you get from
//!    calling `generate_occurrences` / `generate_fees` directly with the same
//!    inputs. No duplicated or divergent logic.
//!
//! 2. **Prev-period boundaries** — the budget comparison correctly handles
//!    months of 28/29/30/31 days (the previous period is the same number of
//!    days ending on the day before the requested period's start).
//!
//! 3. **Performance** — a 12-month period with hundreds of transactions
//!    returns in acceptable time (< 5 s on the in-memory SQLite pool).

use std::collections::BTreeMap;
use std::time::Instant;

use chrono::{Datelike, Duration, NaiveDate};
use pfm_domain::{
    budget_progress, generate_fees, generate_occurrences, Cadence, CalcType, CategoryKind,
    CondBase, EndType, FeePeriod, PeriodMetrics, RecurrenceMode, RecurrenceSpec, WeekendPolicy,
};
use pfm_repo::{DbSettings, Engine, RecurrencesRepo, RegisterFilters, Storage};
use pfm_service::{BalancePoint, RecurrenceCreateCmd, RecurrenceService, StatsService};

const OWNER: &str = "u1";
const ACCOUNT: &str = "acc1";
const CAT_OUT: &str = "cat-food";
const CAT_OUT2: &str = "cat-travel";
const CAT_IN: &str = "cat-salary";

// ── helpers ─────────────────────────────────────────────────────────────────

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

async fn setup() -> (StatsService, RecurrenceService, Storage) {
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

    // users

    // accounts

    // categories

    // account_shares

    // transactions

    // recurrences

    // recurrence_overrides

    // account_rules

    // user_prefs

    // Seed: owner user
    sqlx::query(
        "INSERT INTO users (id, username, email, password_hash, role, active, locale, default_currency, created_at)
         VALUES ('u1', 'owner', 'o@example.com', 'x', 'user', 1, 'en', 'USD', '2025-01-01T00:00:00Z')",
    )
    .execute(pool)
    .await
    .unwrap();

    // Seed: user prefs (show_future = 1)
    sqlx::query(
        "INSERT INTO user_prefs (user_id, theme, show_sign, privacy, show_m_saldo, empty_days, show_future, rows_above, rows_below, rows_step)
         VALUES ('u1', 'system', 1, 0, 0, 0, 1, 2, 8, 1)",
    )
    .execute(pool)
    .await
    .unwrap();

    // Seed: account starting 2022-01-01 with balance 50_000 cents (500.00)
    sqlx::query(
        "INSERT INTO accounts (id, owner_id, name, kind, start_date, start_balance_cents, active, created_at)
         VALUES ('acc1', 'u1', 'conto', 'current', '2022-01-01', 50000, 1, '2025-01-01T00:00:00Z')",
    )
    .execute(pool)
    .await
    .unwrap();

    // Seed: categories
    sqlx::query(
        "INSERT INTO categories (id, key, name, emoji, kind, budget_cents, is_fallback, created_at)
         VALUES ('cat-food', 'food', 'Cibo', '', 'out', 20000, 0, '1970-01-01T00:00:00Z')",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO categories (id, key, name, emoji, kind, budget_cents, is_fallback, created_at)
         VALUES ('cat-travel', 'travel', 'Viaggi', '', 'out', 10000, 0, '1970-01-01T00:00:00Z')",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO categories (id, key, name, emoji, kind, budget_cents, is_fallback, created_at)
         VALUES ('cat-salary', 'salary', 'Stipendio', '', 'in', NULL, 0, '1970-01-01T00:00:00Z')",
    )
    .execute(pool)
    .await
    .unwrap();

    let stats = StatsService::new(&storage);
    let rec_svc = RecurrenceService::new(&storage);
    (stats, rec_svc, storage)
}

/// Insert a single transaction row directly.
async fn insert_tx(
    pool: &sqlx::AnyPool,
    id: &str,
    account: &str,
    date: &str,
    cents: i64,
    category: Option<&str>,
) {
    sqlx::query(
        "INSERT INTO transactions (id, client_id, account_id, date, cents, category_id, created_by, created_at, updated_at)
         VALUES ($1, $1, $2, $3, $4, $5, 'u1', '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z')",
    )
    .bind(id)
    .bind(account)
    .bind(date)
    .bind(cents)
    .bind(category)
    .execute(pool)
    .await
    .unwrap();
}

fn fixed_cmd(pattern: &str, amount: i64, first: &str) -> RecurrenceCreateCmd {
    RecurrenceCreateCmd {
        description: "ricorrente".into(),
        category_id: CAT_OUT.into(),
        kind: CategoryKind::Out,
        amount_cents: amount,
        account_id: ACCOUNT.into(),
        pattern: pattern.into(),
        weekend: WeekendPolicy::Create,
        first_date: NaiveDate::parse_from_str(first, "%Y-%m-%d").unwrap(),
        end_type: EndType::Never,
        end_date: None,
        end_count: None,
        mode: RecurrenceMode::Fixed,
        avg_n: 1,
        active: true,
        note: String::new(),
        linked_account_id: None,
        linked_category_id: None,
    }
}

#[allow(clippy::too_many_arguments)]
async fn insert_rule(
    pool: &sqlx::AnyPool,
    id: &str,
    account: &str,
    category: &str,
    calc_type: &str,
    value: i64,
    cadence: &str,
    charge_day: i64,
    cond_base: &str,
) {
    sqlx::query(
        "INSERT INTO account_rules (id, account_id, name, category_id, calc_type, value, is_annual, cadence, charge_day, cond_base, active, created_at, updated_at)
         VALUES ($1, $2, 'test rule', $3, $4, $5, 0, $6, $7, $8, 1, '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z')",
    )
    .bind(id)
    .bind(account)
    .bind(category)
    .bind(calc_type)
    .bind(value)
    .bind(cadence)
    .bind(charge_day)
    .bind(cond_base)
    .execute(pool)
    .await
    .unwrap();
}

// ── 1. Projection coherence ─────────────────────────────────────────────────
//
// The forecast part of the balance trend must be consistent with direct calls
// to `generate_occurrences` (for recurrences) and `generate_fees` (for fee
// rules) on the same horizon and base balance.

#[tokio::test]
async fn projection_matches_direct_engine_calls() {
    let (stats, rec_svc, storage) = setup().await;
    let pool = storage.pool();

    // Create a monthly recurrence: -1500 on the 15th (fixed, outflow).
    rec_svc
        .create(
            OWNER,
            OWNER,
            &fixed_cmd("FREQ=MONTHLY;BYMONTHDAY=15", -1500, "2024-01-15"),
        )
        .await
        .unwrap();

    // Add a monthly fee rule: -500 on the last day of each month (fixed, always).
    insert_rule(
        pool, "rule1", ACCOUNT, CAT_OUT, "fixed", 500, "monthly", 0, "always",
    )
    .await;

    // Insert one historical transaction (Jan 1, +10000 salary) so the opening
    // balance is non-trivial.
    insert_tx(pool, "tx1", ACCOUNT, "2024-01-01", 10_000, Some(CAT_IN)).await;

    // Request stats with a range that extends well into the future.
    let result = stats
        .stats(
            OWNER,
            ACCOUNT,
            Some(d("2022-01-01")),
            Some(d("2027-12-31")),
            None,
            &RegisterFilters::default(),
        )
        .await
        .unwrap();

    // Find the first forecast point — this is `today + 1` (the start of the
    // projection window).
    let first_forecast = result
        .balance_trend
        .iter()
        .find(|p| p.is_forecast)
        .expect("expected at least one forecast point");
    let proj_from = first_forecast.date;

    // The last historical point before the forecast gives us the base balance.
    let hist_last = result
        .balance_trend
        .iter()
        .rfind(|p| !p.is_forecast)
        .cloned()
        .expect("expected historical points");
    let base_balance = hist_last.balance_cents;

    // Extract the forecast points (the full projection window).
    let forecast_points: Vec<BalancePoint> = result
        .balance_trend
        .iter()
        .filter(|p| p.is_forecast)
        .cloned()
        .collect();
    assert!(
        forecast_points.len() > 30,
        "expected a multi-month projection"
    );

    // Independently compute what the engines would produce starting from
    // `proj_from` with `base_balance`.
    let recs = pfm_repo::SqlxRecurrencesRepo::new(&storage)
        .list_for_account(ACCOUNT)
        .await
        .unwrap();
    let rec = &recs[0];
    let spec: RecurrenceSpec = pfm_service::RecurrenceService::build_spec(rec, Vec::new()).unwrap();

    let rec_occurrences = generate_occurrences(&spec, proj_from, d("2027-12-31"));

    let fee_rule = pfm_domain::FeeRule {
        calc_type: CalcType::Fixed,
        value: 500,
        is_annual: false,
        prorata_days: false,
        annual_cap_cents: None,
        cadence: Cadence::Monthly,
        charge_day: 0,
        charge_month: None,
        cond_base: CondBase::Always,
        cond_op: None,
        cond_threshold: None,
    };
    let fee_rows = generate_fees(&fee_rule, proj_from, d("2027-12-31"), |_p: &FeePeriod| {
        PeriodMetrics::default()
    });

    // Build expected per-day amounts.
    let mut expected_per_day: BTreeMap<NaiveDate, i64> = BTreeMap::new();
    for occ in &rec_occurrences {
        *expected_per_day.entry(occ.date).or_insert(0) += occ.amount_cents;
    }
    for row in &fee_rows {
        *expected_per_day.entry(row.date).or_insert(0) += row.amount_cents;
    }

    // Chain the expected balance and compare with the service's forecast.
    let mut expected_balance = base_balance;
    for fp in &forecast_points {
        if let Some(amt) = expected_per_day.get(&fp.date) {
            expected_balance += amt;
        }
        assert_eq!(
            fp.balance_cents, expected_balance,
            "balance mismatch on {}: service={} direct={}",
            fp.date, fp.balance_cents, expected_balance
        );
    }
}

/// Coherence test with a simpler scenario: only a recurrence, no fee rule.
/// This isolates the recurrence projection path.
#[tokio::test]
async fn projection_recurrence_only_coherence() {
    let (stats, rec_svc, _storage) = setup().await;

    // Monthly -2000 on the 1st.
    rec_svc
        .create(
            OWNER,
            OWNER,
            &fixed_cmd("FREQ=MONTHLY;BYMONTHDAY=1", -2000, "2024-01-01"),
        )
        .await
        .unwrap();

    // No transactions at all (account starts at 50_000).
    // Request a range that extends into the future.
    let result = stats
        .stats(
            OWNER,
            ACCOUNT,
            Some(d("2022-01-01")),
            Some(d("2027-12-31")),
            None,
            &RegisterFilters::default(),
        )
        .await
        .unwrap();

    // Find the first forecast point (today + 1) and the last historical point.
    let first_forecast = result
        .balance_trend
        .iter()
        .find(|p| p.is_forecast)
        .expect("expected forecast points");
    let proj_from = first_forecast.date;

    let hist_last = result
        .balance_trend
        .iter()
        .rfind(|p| !p.is_forecast)
        .cloned()
        .unwrap();
    let base = hist_last.balance_cents;

    let forecast: Vec<BalancePoint> = result
        .balance_trend
        .iter()
        .filter(|p| p.is_forecast)
        .cloned()
        .collect();
    assert!(!forecast.is_empty());

    // Direct engine call: the recurrence produces -2000 on the 1st of each month.
    // Verify coherence by checking that the forecast balance changes by exactly
    // -2000 on each month's 1st and is flat on all other days.
    let mut prev_balance = base;
    let mut prev_date = proj_from - chrono::Duration::days(1);
    for fp in &forecast {
        // Compute what the balance should be.
        if fp.date.day() == 1 && fp.date != proj_from {
            // A recurrence fires on the 1st.
            prev_balance -= 2000;
        }
        // But if proj_from itself is the 1st, the first point already includes it.
        if fp.date == proj_from && fp.date.day() == 1 {
            prev_balance -= 2000;
        }
        assert_eq!(
            fp.balance_cents, prev_balance,
            "balance mismatch on {}: service={} expected={}",
            fp.date, fp.balance_cents, prev_balance
        );
        prev_date = fp.date;
    }
    let _ = prev_date;
}

// ── 2. Prev-period boundary tests ───────────────────────────────────────────
//
// The budget comparison must correctly handle months of different lengths.
// The previous period is the same number of days ending on the day before
// the requested period's start.

/// Helper: insert transactions on specific dates for a category, then
/// verify the budget's spent/prev values match expectations.
#[allow(dead_code)]
#[allow(clippy::too_many_arguments)]
async fn check_budget_spent(
    stats: &StatsService,
    pool: &sqlx::AnyPool,
    tx_date: &str,
    tx_cents: i64,
    from: NaiveDate,
    to: NaiveDate,
    expected_spent: i64,
    expected_prev: i64,
) {
    let id = format!("tx-{}-{}", tx_date, tx_cents);
    insert_tx(pool, &id, ACCOUNT, tx_date, tx_cents, Some(CAT_OUT)).await;

    let result = stats
        .stats(
            OWNER,
            ACCOUNT,
            Some(from),
            Some(to),
            None,
            &RegisterFilters::default(),
        )
        .await
        .unwrap();

    let food_budget = result
        .budget
        .iter()
        .find(|b| b.category_id == CAT_OUT)
        .unwrap_or_else(|| panic!("no budget entry for {CAT_OUT}"));

    assert_eq!(
        food_budget.spent_cents, expected_spent,
        "spent mismatch for period [{from}..={to}] with tx on {tx_date}"
    );
    assert_eq!(
        food_budget.prev_period_cents, expected_prev,
        "prev mismatch for period [{from}..={to}] with tx on {tx_date}"
    );
}

/// January 2024 (31 days): previous period is Dec 1–31, 2023 (31 days).
#[tokio::test]
async fn budget_prev_period_31_day_jan() {
    let (stats, _rec, storage) = setup().await;
    let pool = storage.pool();

    // A tx on Jan 15, 2024 (within the requested period).
    // A tx on Dec 15, 2023 (within the previous period Dec 1–31).
    insert_tx(pool, "tx-jan", ACCOUNT, "2024-01-15", -5000, Some(CAT_OUT)).await;
    insert_tx(pool, "tx-dec", ACCOUNT, "2023-12-15", -3000, Some(CAT_OUT)).await;

    let result = stats
        .stats(
            OWNER,
            ACCOUNT,
            Some(d("2024-01-01")),
            Some(d("2024-01-31")),
            None,
            &RegisterFilters::default(),
        )
        .await
        .unwrap();

    let food = result
        .budget
        .iter()
        .find(|b| b.category_id == CAT_OUT)
        .unwrap();
    assert_eq!(food.spent_cents, 5000);
    assert_eq!(food.prev_period_cents, 3000);
    // budget is 20000, spent is 5000 -> 25%
    assert!((food.pct - 25.0).abs() < 0.01);
}

/// February 2024 (29 days, leap year): previous period is Jan 3–31, 2024 (29 days).
#[tokio::test]
async fn budget_prev_period_29_day_feb_leap() {
    let (stats, _rec, storage) = setup().await;
    let pool = storage.pool();

    // Tx on Feb 15, 2024 (within Feb 1–29).
    insert_tx(pool, "tx-feb", ACCOUNT, "2024-02-15", -4000, Some(CAT_OUT)).await;
    // Tx on Jan 20, 2024 (within Jan 3–31, the prev period).
    insert_tx(
        pool,
        "tx-jan20",
        ACCOUNT,
        "2024-01-20",
        -2000,
        Some(CAT_OUT),
    )
    .await;
    // Tx on Jan 1, 2024 (OUTSIDE the prev period Jan 3–31).
    insert_tx(pool, "tx-jan1", ACCOUNT, "2024-01-01", -9999, Some(CAT_OUT)).await;

    let result = stats
        .stats(
            OWNER,
            ACCOUNT,
            Some(d("2024-02-01")),
            Some(d("2024-02-29")),
            None,
            &RegisterFilters::default(),
        )
        .await
        .unwrap();

    let food = result
        .budget
        .iter()
        .find(|b| b.category_id == CAT_OUT)
        .unwrap();
    assert_eq!(food.spent_cents, 4000);
    // Only Jan 20 is in the prev period (Jan 3–31); Jan 1 is excluded.
    assert_eq!(food.prev_period_cents, 2000);
}

/// February 2023 (28 days, non-leap): previous period is Jan 4–31, 2023 (28 days).
#[tokio::test]
async fn budget_prev_period_28_day_feb_nonleap() {
    let (stats, _rec, storage) = setup().await;
    let pool = storage.pool();

    // Tx on Feb 10, 2023 (within Feb 1–28).
    insert_tx(
        pool,
        "tx-feb23",
        ACCOUNT,
        "2023-02-10",
        -3000,
        Some(CAT_OUT),
    )
    .await;
    // Tx on Jan 20, 2023 (within Jan 4–31, the prev period).
    insert_tx(
        pool,
        "tx-jan23",
        ACCOUNT,
        "2023-01-20",
        -1500,
        Some(CAT_OUT),
    )
    .await;
    // Tx on Jan 2, 2023 (OUTSIDE the prev period Jan 4–31).
    insert_tx(pool, "tx-jan2", ACCOUNT, "2023-01-02", -8888, Some(CAT_OUT)).await;

    let result = stats
        .stats(
            OWNER,
            ACCOUNT,
            Some(d("2023-02-01")),
            Some(d("2023-02-28")),
            None,
            &RegisterFilters::default(),
        )
        .await
        .unwrap();

    let food = result
        .budget
        .iter()
        .find(|b| b.category_id == CAT_OUT)
        .unwrap();
    assert_eq!(food.spent_cents, 3000);
    // Only Jan 20 is in the prev period (Jan 4–31); Jan 2 is excluded.
    assert_eq!(food.prev_period_cents, 1500);
}

/// April 2024 (30 days): previous period is Mar 2–31, 2024 (30 days).
#[tokio::test]
async fn budget_prev_period_30_day_april() {
    let (stats, _rec, storage) = setup().await;
    let pool = storage.pool();

    // Tx on Apr 15, 2024 (within Apr 1–30).
    insert_tx(pool, "tx-apr", ACCOUNT, "2024-04-15", -6000, Some(CAT_OUT)).await;
    // Tx on Mar 20, 2024 (within Mar 2–31, the prev period).
    insert_tx(
        pool,
        "tx-mar20",
        ACCOUNT,
        "2024-03-20",
        -2500,
        Some(CAT_OUT),
    )
    .await;
    // Tx on Mar 1, 2024 (OUTSIDE the prev period Mar 2–31).
    insert_tx(pool, "tx-mar1", ACCOUNT, "2024-03-01", -7777, Some(CAT_OUT)).await;

    let result = stats
        .stats(
            OWNER,
            ACCOUNT,
            Some(d("2024-04-01")),
            Some(d("2024-04-30")),
            None,
            &RegisterFilters::default(),
        )
        .await
        .unwrap();

    let food = result
        .budget
        .iter()
        .find(|b| b.category_id == CAT_OUT)
        .unwrap();
    assert_eq!(food.spent_cents, 6000);
    // Only Mar 20 is in the prev period (Mar 2–31); Mar 1 is excluded.
    assert_eq!(food.prev_period_cents, 2500);
}

/// June 2024 (30 days): previous period is May 2–31, 2024 (30 days).
#[tokio::test]
async fn budget_prev_period_30_day_june() {
    let (stats, _rec, storage) = setup().await;
    let pool = storage.pool();

    insert_tx(pool, "tx-jun", ACCOUNT, "2024-06-15", -1000, Some(CAT_OUT)).await;
    insert_tx(pool, "tx-may", ACCOUNT, "2024-05-15", -2000, Some(CAT_OUT)).await;

    let result = stats
        .stats(
            OWNER,
            ACCOUNT,
            Some(d("2024-06-01")),
            Some(d("2024-06-30")),
            None,
            &RegisterFilters::default(),
        )
        .await
        .unwrap();

    let food = result
        .budget
        .iter()
        .find(|b| b.category_id == CAT_OUT)
        .unwrap();
    assert_eq!(food.spent_cents, 1000);
    assert_eq!(food.prev_period_cents, 2000);
}

/// Year boundary: January 2025 (31 days), prev period is Dec 1–31, 2024.
#[tokio::test]
async fn budget_prev_period_cross_year() {
    let (stats, _rec, storage) = setup().await;
    let pool = storage.pool();

    insert_tx(
        pool,
        "tx-jan25",
        ACCOUNT,
        "2025-01-15",
        -5000,
        Some(CAT_OUT),
    )
    .await;
    insert_tx(
        pool,
        "tx-dec24",
        ACCOUNT,
        "2024-12-15",
        -4000,
        Some(CAT_OUT),
    )
    .await;

    let result = stats
        .stats(
            OWNER,
            ACCOUNT,
            Some(d("2025-01-01")),
            Some(d("2025-01-31")),
            None,
            &RegisterFilters::default(),
        )
        .await
        .unwrap();

    let food = result
        .budget
        .iter()
        .find(|b| b.category_id == CAT_OUT)
        .unwrap();
    assert_eq!(food.spent_cents, 5000);
    assert_eq!(food.prev_period_cents, 4000);
}

// ── 3. Performance test ─────────────────────────────────────────────────────
//
// A 12-month period with hundreds of transactions must return in acceptable
// time. We use 500 transactions spread across 12 months and assert the
// response time is under 5 seconds (generous for in-memory SQLite).

#[tokio::test]
async fn performance_12_months_500_tx() {
    let (stats, _rec, storage) = setup().await;
    let pool = storage.pool();

    // Insert 500 transactions spread across 2024 (Jan–Dec).
    // ~42 per month, alternating in/out, across two categories.
    let mut tx_id = 0u32;
    let mut day = 1u32;
    for month in 1u32..=12 {
        for _ in 0..42 {
            tx_id += 1;
            let date_str = format!("2024-{month:02}-{day:02}");
            let cents = if tx_id % 2 == 0 {
                -((tx_id as i64) * 100 + 50) // outflow
            } else {
                (tx_id as i64) * 100 + 50 // inflow
            };
            let cat = if tx_id % 3 == 0 {
                Some(CAT_OUT2)
            } else {
                Some(CAT_OUT)
            };
            let id = format!("ptx-{tx_id}");
            sqlx::query(
                "INSERT INTO transactions (id, client_id, account_id, date, cents, category_id, created_by, created_at, updated_at)
                 VALUES ($1, $1, $2, $3, $4, $5, 'u1', '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z')",
            )
            .bind(&id)
            .bind(ACCOUNT)
            .bind(&date_str)
            .bind(cents)
            .bind(cat)
            .execute(pool)
            .await
            .unwrap();
            day += 1;
            if day > 28 {
                day = 1;
            }
        }
    }

    // Time the stats call for the full year 2024.
    let start = Instant::now();
    let result = stats
        .stats(
            OWNER,
            ACCOUNT,
            Some(d("2024-01-01")),
            Some(d("2024-12-31")),
            None,
            &RegisterFilters::default(),
        )
        .await
        .unwrap();
    let elapsed = start.elapsed();

    // Assert reasonable performance: under 5 seconds.
    assert!(
        elapsed.as_secs() < 5,
        "stats took {elapsed:?} for 500 tx over 12 months (limit: 5s)"
    );

    // Sanity: the KPIs should be non-trivial.
    assert!(result.kpi.income_cents > 0);
    assert!(result.kpi.expense_cents > 0);
    assert!(!result.balance_trend.is_empty());
    assert!(!result.expenses_by_category.is_empty());

    // The balance trend should cover the full year.
    assert_eq!(result.balance_trend.len(), 366); // 2024 is a leap year

    // Budget should have entries for both out categories.
    assert!(!result.budget.is_empty());
}

/// Performance with recurrences AND fee rules AND many transactions — the
/// full pipeline (historical + projection + budget) must still be fast.
#[tokio::test]
async fn performance_full_pipeline() {
    let (stats, rec_svc, storage) = setup().await;
    let pool = storage.pool();

    // Create a daily recurrence (high frequency to stress the projection).
    rec_svc
        .create(OWNER, OWNER, &fixed_cmd("FREQ=DAILY", -100, "2024-01-01"))
        .await
        .unwrap();

    // Add a monthly fee rule.
    insert_rule(
        pool,
        "rule-perf",
        ACCOUNT,
        CAT_OUT,
        "fixed",
        200,
        "monthly",
        0,
        "always",
    )
    .await;

    // 400 transactions over 2024.
    for i in 0..400u32 {
        let month = ((i as usize) % 12) + 1;
        let day = ((i as usize) % 28) + 1;
        let date_str = format!("2024-{month:02}-{day:02}");
        let id = format!("fptx-{i}");
        let cents = if i % 2 == 0 {
            -(i as i64 * 50 + 100)
        } else {
            i as i64 * 50 + 100
        };
        sqlx::query(
            "INSERT INTO transactions (id, client_id, account_id, date, cents, category_id, created_by, created_at, updated_at)
             VALUES ($1, $1, $2, $3, $4, $5, 'u1', '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z')",
        )
        .bind(&id)
        .bind(ACCOUNT)
        .bind(&date_str)
        .bind(cents)
        .bind(CAT_OUT)
        .execute(pool)
        .await
        .unwrap();
    }

    // Full year + 3 months of projection (extends past today for 2026).
    let start = Instant::now();
    let _result = stats
        .stats(
            OWNER,
            ACCOUNT,
            Some(d("2024-01-01")),
            Some(d("2026-03-31")),
            None,
            &RegisterFilters::default(),
        )
        .await
        .unwrap();
    let elapsed = start.elapsed();

    assert!(
        elapsed.as_secs() < 10,
        "full pipeline took {elapsed:?} (limit: 10s)"
    );
}

// ── Additional edge-case tests ──────────────────────────────────────────────

/// Empty period (no transactions, no recurrences, no rules): KPIs are zero,
/// budget shows zero spent, balance trend is flat.
#[tokio::test]
async fn empty_period_zero_kpis() {
    let (stats, _rec, _storage) = setup().await;

    let result = stats
        .stats(
            OWNER,
            ACCOUNT,
            Some(d("2024-03-01")),
            Some(d("2024-03-31")),
            None,
            &RegisterFilters::default(),
        )
        .await
        .unwrap();

    assert_eq!(result.kpi.income_cents, 0);
    assert_eq!(result.kpi.expense_cents, 0);
    assert_eq!(result.kpi.net_cents, 0);
    // Opening = start_balance + sum before = 50000 + 0 = 50000.
    assert_eq!(result.kpi.opening_cents, 50_000);
    assert_eq!(result.kpi.closing_cents, 50_000);
    assert!(result.expenses_by_category.is_empty());
    // Budget: spent = 0, prev = 0, pct = 0.
    let food = result
        .budget
        .iter()
        .find(|b| b.category_id == CAT_OUT)
        .unwrap();
    assert_eq!(food.spent_cents, 0);
    assert_eq!(food.prev_period_cents, 0);
    assert_eq!(food.pct, 0.0);
}

/// The `cat` filter narrows expenses_by_category but NOT the KPIs or budget.
#[tokio::test]
async fn cat_filter_affects_only_expenses() {
    let (stats, _rec, storage) = setup().await;
    let pool = storage.pool();

    // One tx in CAT_OUT, one in CAT_OUT2.
    insert_tx(pool, "tx-food", ACCOUNT, "2024-01-15", -5000, Some(CAT_OUT)).await;
    insert_tx(
        pool,
        "tx-travel",
        ACCOUNT,
        "2024-01-20",
        -3000,
        Some(CAT_OUT2),
    )
    .await;

    // Unfiltered: both categories appear.
    let full = stats
        .stats(
            OWNER,
            ACCOUNT,
            Some(d("2024-01-01")),
            Some(d("2024-01-31")),
            None,
            &RegisterFilters::default(),
        )
        .await
        .unwrap();
    assert_eq!(full.expenses_by_category.len(), 2);
    // KPIs include both.
    assert_eq!(full.kpi.expense_cents, 8000);

    // Filtered to CAT_OUT only.
    let filtered = stats
        .stats(
            OWNER,
            ACCOUNT,
            Some(d("2024-01-01")),
            Some(d("2024-01-31")),
            None,
            &RegisterFilters {
                category_id: Some(CAT_OUT.into()),
                ..RegisterFilters::default()
            },
        )
        .await
        .unwrap();
    // expenses_by_category is narrowed.
    assert_eq!(filtered.expenses_by_category.len(), 1);
    assert_eq!(filtered.expenses_by_category[0].category_id, CAT_OUT);
    // KPIs are still from the full ledger (unfiltered).
    assert_eq!(filtered.kpi.expense_cents, 8000);
    // Budget is also from the full ledger.
    let food = filtered
        .budget
        .iter()
        .find(|b| b.category_id == CAT_OUT)
        .unwrap();
    assert_eq!(food.spent_cents, 5000);
}

/// Budget with over-spending: pct > 100.
#[tokio::test]
async fn budget_over_100_percent() {
    let (stats, _rec, storage) = setup().await;
    let pool = storage.pool();

    // Spend 25000 (budget is 20000) -> 125%.
    insert_tx(
        pool,
        "tx-overspend",
        ACCOUNT,
        "2024-01-15",
        -25000,
        Some(CAT_OUT),
    )
    .await;

    let result = stats
        .stats(
            OWNER,
            ACCOUNT,
            Some(d("2024-01-01")),
            Some(d("2024-01-31")),
            None,
            &RegisterFilters::default(),
        )
        .await
        .unwrap();

    let food = result
        .budget
        .iter()
        .find(|b| b.category_id == CAT_OUT)
        .unwrap();
    assert_eq!(food.spent_cents, 25000);
    assert!((food.pct - 125.0).abs() < 0.01);
}

/// Verify that `prev_period_range` handles all month lengths correctly at the
/// domain level. For every month transition we verify:
/// 1. The previous period has the same number of days as the current period.
/// 2. `prev_to` is the day immediately before `from`.
/// 3. `prev_from` is `prev_to - (days - 1)`.
#[test]
fn prev_period_all_month_transitions() {
    // Test every month of 2024 (leap year) and 2023 (non-leap).
    for year in [2023, 2024] {
        for month in 1u32..=12 {
            let from = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
            let to = if month == 12 {
                NaiveDate::from_ymd_opt(year + 1, 1, 1).unwrap() - Duration::days(1)
            } else {
                NaiveDate::from_ymd_opt(year, month + 1, 1).unwrap() - Duration::days(1)
            };

            let days = (to - from).num_days() + 1;
            let (pf, pt) = pfm_domain::prev_period_range(from, to);

            // Invariant 1: same number of days.
            assert_eq!(
                (pt - pf).num_days() + 1,
                days,
                "month {year}-{month:02} ({days}d): prev period has different length"
            );
            // Invariant 2: prev_to = from - 1 day.
            assert_eq!(
                pt,
                from - Duration::days(1),
                "month {year}-{month:02}: prev_to should be {from} - 1 day"
            );
            // Invariant 3: prev_from = prev_to - (days - 1).
            assert_eq!(
                pf,
                pt - Duration::days(days - 1),
                "month {year}-{month:02}: prev_from calculation mismatch"
            );
            // The prev period must end before the current period starts.
            assert!(pt < from);
        }
    }
}

/// The budget engine's `budget_progress` correctly computes pct for the
/// boundary cases where spent exactly equals budget (100%).
#[test]
fn budget_progress_exact_100() {
    let p = budget_progress("c", "C", 10_000, 10_000, 5_000);
    assert!((p.pct - 100.0).abs() < f64::EPSILON);
}

/// A recurrence with an end date that falls within the projection horizon:
/// occurrences after the end date must not appear in the forecast.
#[tokio::test]
async fn projection_respects_recurrence_end_date() {
    let (stats, _rec, storage) = setup().await;
    let _pool = storage.pool();

    // Create a monthly recurrence that ends on 2026-10-15.
    let cmd = RecurrenceCreateCmd {
        description: "limited".into(),
        category_id: CAT_OUT.into(),
        kind: CategoryKind::Out,
        amount_cents: -1000,
        account_id: ACCOUNT.into(),
        pattern: "FREQ=MONTHLY;BYMONTHDAY=15".into(),
        weekend: WeekendPolicy::Create,
        first_date: d("2024-01-15"),
        end_type: EndType::Until,
        end_date: Some(d("2026-10-15")),
        end_count: None,
        mode: RecurrenceMode::Fixed,
        avg_n: 1,
        active: true,
        note: String::new(),
        linked_account_id: None,
        linked_category_id: None,
    };
    pfm_service::RecurrenceService::new(&storage)
        .create(OWNER, OWNER, &cmd)
        .await
        .unwrap();

    let result = stats
        .stats(
            OWNER,
            ACCOUNT,
            Some(d("2026-09-01")),
            Some(d("2026-12-31")),
            None,
            &RegisterFilters::default(),
        )
        .await
        .unwrap();

    // The forecast should include Oct 15 but NOT Nov 15 or Dec 15
    // (the recurrence ended on Oct 15).
    let forecast: Vec<BalancePoint> = result
        .balance_trend
        .iter()
        .filter(|p| p.is_forecast)
        .cloned()
        .collect();

    // Find the balance at Oct 15 and Nov 15.
    let before_oct = forecast
        .iter()
        .find(|p| p.date == d("2026-10-14"))
        .map(|p| p.balance_cents);
    let at_oct15 = forecast
        .iter()
        .find(|p| p.date == d("2026-10-15"))
        .map(|p| p.balance_cents);
    let at_nov15 = forecast
        .iter()
        .find(|p| p.date == d("2026-11-15"))
        .map(|p| p.balance_cents);
    let at_dec15 = forecast
        .iter()
        .find(|p| p.date == d("2026-12-15"))
        .map(|p| p.balance_cents);

    if let (Some(b), Some(a)) = (before_oct, at_oct15) {
        assert_eq!(a, b - 1000, "Oct 15 should drop by 1000 (last occurrence)");
    }
    if let (Some(o), Some(n)) = (at_oct15, at_nov15) {
        assert_eq!(n, o, "Nov 15 should NOT change (recurrence ended)");
    }
    if let (Some(n), Some(dec)) = (at_nov15, at_dec15) {
        assert_eq!(dec, n, "Dec 15 should NOT change (recurrence ended)");
    }
}
