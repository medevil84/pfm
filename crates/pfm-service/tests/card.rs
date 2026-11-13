//! Service-level gate tests for credit-card settlement. The gates pin the core
//! invariants of the end-of-cycle charge:
//!
//!  1. The charge is the *net sum* of the previous cycle's raw card movements
//!     (not the gross, not a fixed value) — proved by a dry-run that bills the
//!     exact net outflow without writing anything.
//!  2. The card's remaining limit resets to its full limit right after the
//!     settlement posts (the credit cancels the outstanding balance).
//!  3. The charge is never a fixed amount: two consecutive cycles with
//!     different spend yield different charges (cycle N+1 bills cycle N).

use chrono::NaiveDate;
use pfm_repo::{DbSettings, Engine, SqlxTransactionsRepo, Storage, TransactionsRepo};
use pfm_service::{CardCycleDays, CardService};

const OWNER: &str = "u1";
const BACKING: &str = "acc-backing";
const CARD: &str = "acc-card";
const LIMIT: i64 = 100_000;

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

async fn setup() -> (CardService, Storage) {
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

    // Seed the owner and, for it, a backing current account plus a card that
    // charges on the 5th against that backing account (limit 1,000.00).
    sqlx::query(
        "INSERT INTO users (id, username, email, password_hash, role, active, locale, default_currency, created_at)
         VALUES ('u1', 'owner', 'o@example.com', 'x', 'user', 1, 'en', 'USD', '2024-01-01T00:00:00Z')",
    )
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO accounts (id, owner_id, name, kind, start_date, start_balance_cents, active, created_at)
         VALUES ('acc-backing', 'u1', 'checking', 'current', '2024-01-01', 0, 1, '2024-01-01T00:00:00Z')",
    )
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO accounts (id, owner_id, name, kind, start_date, start_balance_cents,
              limit_cents, charge_day, primary_account_id, active, created_at)
         VALUES ('acc-card', 'u1', 'plastic', 'card', '2024-01-01', 0, $1, 5, 'acc-backing', 1, '2024-01-01T00:00:00Z')",
    )
    .bind(LIMIT)
    .execute(pool)
    .await
    .unwrap();

    let svc = CardService::new(&storage);
    (svc, storage)
}

/// Insert a raw (non-settlement) movement on the card.
async fn seed_raw(pool: &sqlx::AnyPool, id: &str, date: &str, cents: i64) {
    sqlx::query(
        "INSERT INTO transactions \
         (id, client_id, account_id, date, cents, description, created_by, created_at, updated_at) \
         VALUES ($1, $1, $2, $3, $4, $5, $6, $7, $7)",
    )
    .bind(id)
    .bind(CARD)
    .bind(date)
    .bind(cents)
    .bind("raw")
    .bind(OWNER)
    .bind("2024-01-01T00:00:00Z")
    .execute(pool)
    .await
    .unwrap();
}

/// Gate 1 + 2: the charge is the net sum of the previous cycle's raw movements
/// (a dry-run bills it without writing), and the remaining limit resets to the
/// full limit once the settlement actually posts.
#[tokio::test]
async fn charge_is_net_sum_and_resets_residual() {
    let (svc, storage) = setup().await;
    let pool = storage.pool();

    // The cycle for as_of 2024-03-05 (charge_day 5) is [2024-02-05,
    // 2024-03-05): a 150.00 purchase and a 50.00 refund net to a 100.00
    // outflow.
    seed_raw(pool, "r1", "2024-02-10", -15_000).await;
    seed_raw(pool, "r2", "2024-02-20", 5_000).await;

    // Dry-run: bills the exact net (100.00), not the gross 150.00, and writes
    // no settlement. The limit reflects the 100.00 still outstanding.
    let preview = svc
        .catch_up_full(OWNER, CARD, d("2024-03-05"), true)
        .await
        .unwrap();
    assert_eq!(preview.latest_charge_date, d("2024-03-05"));
    assert_eq!(
        preview.total_charge_cents, 10_000,
        "charge must be the net sum, not the gross"
    );
    assert!(
        preview.pair_ids.is_empty(),
        "dry-run must not write a settlement"
    );
    assert_eq!(preview.available_cents, Some(LIMIT - 10_000));

    // The real settlement posts the pair and the residual limit resets to full.
    let settled = svc
        .catch_up_full(OWNER, CARD, d("2024-03-05"), false)
        .await
        .unwrap();
    assert_eq!(settled.total_charge_cents, 10_000);
    assert!(!settled.pair_ids.is_empty(), "settlement must be written");
    assert_eq!(settled.pair_ids[0], format!("cc:{CARD}:2024-03"));
    assert_eq!(
        settled.available_cents,
        Some(LIMIT),
        "residual must reset after the settlement posts"
    );

    // The pair is exactly two linked legs: backing debited, card credited.
    let legs = SqlxTransactionsRepo::new(&storage)
        .get_transfer_legs(&settled.pair_ids[0])
        .await
        .unwrap();
    let by_account: Vec<(String, i64)> = legs
        .iter()
        .map(|l| (l.account_id.clone(), l.cents))
        .collect();
    assert_eq!(
        by_account,
        vec![(BACKING.to_string(), -10_000), (CARD.to_string(), 10_000)]
    );
}

/// Gate 3: the charge is never a fixed amount. Two consecutive cycles with
/// different spend produce different charges — each bills its own cycle's
/// actual net outflow (cycle N+1 bills cycle N).
#[tokio::test]
async fn charge_tracks_actual_spend_not_fixed() {
    let (svc, storage) = setup().await;
    let pool = storage.pool();

    // Cycle 1 [2024-02-05, 2024-03-05): a 150.00 spend and a 50.00 refund net
    // to a 100.00 outflow.
    seed_raw(pool, "c1a", "2024-02-10", -15_000).await;
    seed_raw(pool, "c1b", "2024-02-20", 5_000).await;
    let first = svc
        .catch_up_full(OWNER, CARD, d("2024-03-05"), false)
        .await
        .unwrap();
    assert_eq!(first.total_charge_cents, 10_000);

    // Cycle 2 [2024-03-05, 2024-04-05): a different 250.00 outflow. The prior
    // cycle's settlement leg (dated 2024-03-05, carrying a pair_id) is excluded
    // from this cycle's raw sum.
    seed_raw(pool, "c2a", "2024-03-15", -25_000).await;
    let second = svc
        .catch_up_full(OWNER, CARD, d("2024-04-05"), false)
        .await
        .unwrap();
    // Full sweep re-sets all cycles: 10000 (Feb) + 25000 (Mar) = 35000.
    assert_eq!(second.total_charge_cents, 35_000);

    assert_ne!(
        first.total_charge_cents, second.total_charge_cents,
        "charge must track each cycle's spend, not be a fixed value"
    );
}

/// Gate 4: backfill. Import transactions from a past month and the catch-up
/// settles all cycles at once.
#[tokio::test]
async fn backfill_settles_all_arrears() {
    let (svc, storage) = setup().await;
    let pool = storage.pool();

    // Import txs from December 2024.
    seed_raw(pool, "d1", "2024-12-10", -50_000).await;
    seed_raw(pool, "d2", "2024-12-20", -30_000).await;

    // Catch up as of 2025-01-10: all cycles from Jan to Jan should be settled.
    let result = svc
        .catch_up_full(OWNER, CARD, d("2025-01-10"), false)
        .await
        .unwrap();

    // Cycles due: 2024-01-05, 02-05, 03-05, ..., 2025-01-05 = 13 cycles.
    // Only Dec has txs: -50000 - 30000 = -80000 → charge 80000.
    assert_eq!(result.total_charge_cents, 80_000);
    assert!(result.cycles_settled >= 13);

    // The limit should be at full (all settled).
    assert_eq!(result.available_cents, Some(LIMIT));
}

/// Gate 5: when a cycle's net drops to zero (full refund), the prior
/// settlement is removed.
#[tokio::test]
async fn zero_charge_removes_prior_settlement() {
    let (svc, storage) = setup().await;
    let pool = storage.pool();

    // First: settle a cycle with a charge.
    seed_raw(pool, "z1", "2024-02-10", -10_000).await;
    let first = svc
        .catch_up_full(OWNER, CARD, d("2024-03-05"), false)
        .await
        .unwrap();
    assert_eq!(first.total_charge_cents, 10_000);
    assert!(!first.pair_ids.is_empty());

    // Now add a refund that makes the cycle net to zero.
    seed_raw(pool, "z2", "2024-02-15", 10_000).await;

    // Re-settle: the cycle now nets to zero, so the pair should be deleted.
    let second = svc
        .catch_up_full(OWNER, CARD, d("2024-03-05"), false)
        .await
        .unwrap();
    assert_eq!(second.total_charge_cents, 0);
    assert!(
        second.pair_ids.is_empty(),
        "zero-charge cycle writes no new pair"
    );

    // Verify the pair is actually gone from the DB.
    let legs = SqlxTransactionsRepo::new(&storage)
        .get_transfer_legs(&format!("cc:{CARD}:2024-03"))
        .await
        .unwrap();
    assert!(legs.is_empty(), "prior settlement pair must be removed");
}

/// Gate 6: delta settlement via the transaction service (simulated by
/// calling settle_delta directly).
#[tokio::test]
async fn settle_delta_recomputes_affected_cycle() {
    let (svc, storage) = setup().await;
    let pool = storage.pool();

    // Seed a tx in Feb.
    seed_raw(pool, "t1", "2024-02-10", -20_000).await;

    // First full sweep settles it.
    let _ = svc
        .catch_up_full(OWNER, CARD, d("2024-03-05"), false)
        .await
        .unwrap();

    // Now add another tx in the same cycle via the delta path.
    seed_raw(pool, "t2", "2024-02-20", -10_000).await;

    // Trigger delta: the affected date is 2024-02-20, which falls in
    // [2024-02-05, 2024-03-05).
    svc.settle_delta(
        CARD,
        CardCycleDays {
            closing_day: 5,
            charge_day: 5,
        },
        d("2024-01-01"),
        d("2024-03-05"),
        BACKING,
        &[d("2024-02-20")],
    )
    .await
    .unwrap();

    // The settlement pair should now reflect the updated charge: 30000.
    let legs = SqlxTransactionsRepo::new(&storage)
        .get_transfer_legs(&format!("cc:{CARD}:2024-03"))
        .await
        .unwrap();
    assert_eq!(legs.len(), 2);
    let backing_leg = legs.iter().find(|l| l.account_id == BACKING).unwrap();
    assert_eq!(backing_leg.cents, -30_000);
}

/// Closing on a weekend: the card's reset credit is dated on the cycle's last
/// day (the Friday before) so the statement stays in the month it closes, and
/// the weekend's spends roll into the next cycle. A credit written before the
/// rule, dated on the nominal weekend closing date, is moved on the next sweep.
#[tokio::test]
async fn weekend_closing_credit_stays_in_its_own_cycle() {
    let (svc, storage) = setup().await;
    let pool = storage.pool();
    // Closing day 4, charge day 17: 2026-01-04 is a Sunday.
    sqlx::query("UPDATE accounts SET closing_day = 4, charge_day = 17 WHERE id = 'acc-card'")
        .execute(pool)
        .await
        .unwrap();
    seed_raw(pool, "w1", "2025-12-20", -20_000).await;
    seed_raw(pool, "w2", "2026-01-02", -10_000).await; // Friday: December cycle
    seed_raw(pool, "w3", "2026-01-03", -5_000).await; // Saturday: January cycle

    let out = svc
        .catch_up_full(OWNER, CARD, d("2026-01-20"), false)
        .await
        .unwrap();
    assert_eq!(out.total_charge_cents, 30_000);
    let pid = out.pair_ids.last().unwrap().clone();

    let repo = SqlxTransactionsRepo::new(&storage);
    let card_leg = |legs: Vec<pfm_domain::Transaction>| {
        legs.into_iter().find(|l| l.account_id == CARD).unwrap()
    };
    let leg = card_leg(repo.get_transfer_legs(&pid).await.unwrap());
    assert_eq!((leg.date, leg.cents), (d("2026-01-02"), 30_000));

    // Legacy row: credit dated on the Sunday.
    sqlx::query("UPDATE transactions SET date = '2026-01-04' WHERE id = $1")
        .bind(&leg.id)
        .execute(pool)
        .await
        .unwrap();
    svc.catch_up_full(OWNER, CARD, d("2026-01-20"), false)
        .await
        .unwrap();
    let leg = card_leg(repo.get_transfer_legs(&pid).await.unwrap());
    assert_eq!(leg.date, d("2026-01-02"));
}

/// Forecast of the cards' not-yet-settled cycles: a cycle closed but not yet
/// charged and the open one, each with its net spend (future-dated spend
/// included), the card credit on the cycle's last day and the backing debit on
/// the business-day-adjusted charge date. A settled cycle is not forecast.
#[tokio::test]
async fn forecast_lists_unsettled_cycles() {
    let (svc, storage) = setup().await;
    let pool = storage.pool();
    sqlx::query("UPDATE accounts SET closing_day = 4, charge_day = 17 WHERE id = 'acc-card'")
        .execute(pool)
        .await
        .unwrap();
    seed_raw(pool, "f1", "2025-12-20", -20_000).await; // December cycle (closes 4 Jan)
    seed_raw(pool, "f2", "2026-01-08", -12_000).await; // current cycle (closes 4 Feb)
    seed_raw(pool, "f3", "2026-01-20", -3_000).await; // future-dated, same cycle

    let today = d("2026-01-10");
    let list = svc
        .forecast(OWNER, d("2026-01-01"), d("2026-03-01"), today)
        .await
        .unwrap();
    let summary: Vec<_> = list
        .iter()
        .map(|f| (f.card_date, f.backing_date, f.spend_cents))
        .collect();
    assert_eq!(
        summary,
        vec![
            // Closed on Sunday 4 Jan (credit on Friday 2 Jan), charged Saturday
            // 17 Jan → Monday 19 Jan.
            (d("2026-01-02"), d("2026-01-19"), 20_000),
            (d("2026-02-04"), d("2026-02-17"), 15_000),
        ]
    );

    // Once the first cycle is really settled it is no longer a forecast.
    svc.catch_up_full(OWNER, CARD, d("2026-01-20"), false)
        .await
        .unwrap();
    let list = svc
        .forecast(OWNER, d("2026-01-01"), d("2026-03-01"), d("2026-01-20"))
        .await
        .unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].spend_cents, 15_000);
}
