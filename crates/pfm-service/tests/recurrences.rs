//! Service-level gate tests for recurrence rules: override stability across
//! regeneration, average-mode auto-update when a new override is confirmed, and
//! the "update rule" flag re-basing a fixed rule's amount for future
//! occurrences.

use chrono::NaiveDate;
use pfm_domain::{CategoryKind, EndType, RecurrenceMode, RecurrenceOccurrence, WeekendPolicy};
use pfm_repo::{DbSettings, Engine, SqlxTransactionsRepo, Storage, TransactionsRepo};
use pfm_service::{
    OverrideCmd, RecurrenceCreateCmd, RecurrenceError, RecurrenceService, RecurrenceUpdateCmd,
};

const OWNER: &str = "u1";
const ACCOUNT: &str = "acc1";
const CAT_OUT: &str = "cat-fallback-out";

async fn setup() -> (RecurrenceService, Storage) {
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

    // Seed owner, an owned account, and the fallback-out category.
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

    let svc = RecurrenceService::new(&storage);
    (svc, storage)
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

fn avg_cmd(pattern: &str, avg_n: i64, first: &str) -> RecurrenceCreateCmd {
    RecurrenceCreateCmd {
        description: "media".into(),
        category_id: CAT_OUT.into(),
        kind: CategoryKind::Out,
        amount_cents: 0,
        account_id: ACCOUNT.into(),
        pattern: pattern.into(),
        weekend: WeekendPolicy::Create,
        first_date: NaiveDate::parse_from_str(first, "%Y-%m-%d").unwrap(),
        end_type: EndType::Never,
        end_date: None,
        end_count: None,
        mode: RecurrenceMode::Avg,
        avg_n,
        active: true,
        note: String::new(),
        linked_account_id: None,
        linked_category_id: None,
    }
}

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

fn by_date<'a>(occs: &'a [RecurrenceOccurrence], date: &str) -> &'a RecurrenceOccurrence {
    let target = d(date);
    occs.iter()
        .find(|o| o.date == target)
        .unwrap_or_else(|| panic!("no occurrence on {date}; got {:?}", occs))
}

/// Gate 1: a date confirmed via override is pinned. Regenerating the horizon
/// repeatedly (the rest of the rule is re-generated on every pass) never changes
/// the confirmed value; the surrounding unconfirmed months keep re-estimating.
#[tokio::test]
async fn override_is_stable_across_regeneration() {
    let (svc, _store) = setup().await;
    let rec = svc
        .create(
            OWNER,
            OWNER,
            &fixed_cmd("FREQ=MONTHLY;BYMONTHDAY=15", 1000, "2024-01-15"),
        )
        .await
        .unwrap();
    let rid = &rec.id;

    // Confirm February with a different real amount.
    svc.set_override(
        OWNER,
        rid,
        d("2024-02-15"),
        &OverrideCmd {
            amount_cents: 1500,
            description: Some("feb reale".into()),
            update_rule: false,
        },
    )
    .await
    .unwrap();

    // Regenerate the horizon several times. The confirmed date must keep its
    // value on every pass, while the unconfirmed months stay at the rule amount.
    for _ in 0..5 {
        let occs = svc
            .occurrences(OWNER, rid, d("2024-01-01"), d("2024-03-31"))
            .await
            .unwrap();
        assert_eq!(occs.len(), 3, "expected Jan/Feb/Mar: {occs:?}");

        let feb = by_date(&occs, "2024-02-15");
        assert_eq!(
            feb.amount_cents, 1500,
            "confirmed value drifted on a re-generation"
        );
        assert!(feb.overridden);

        let jan = by_date(&occs, "2024-01-15");
        let mar = by_date(&occs, "2024-03-15");
        assert_eq!(jan.amount_cents, 1000);
        assert!(!jan.overridden);
        assert_eq!(mar.amount_cents, 1000);
        assert!(!mar.overridden);
    }
}

/// Gate: in `avg` mode, confirming a new real occurrence feeds the average for
/// later (unconfirmed) occurrences, so their estimated amount changes.
#[tokio::test]
async fn avg_updates_when_new_override_confirmed() {
    let (svc, _store) = setup().await;
    let rec = svc
        .create(
            OWNER,
            OWNER,
            &avg_cmd("FREQ=MONTHLY;BYMONTHDAY=1", 2, "2024-01-01"),
        )
        .await
        .unwrap();
    let rid = &rec.id;

    // Two confirmed real occurrences to seed the average.
    svc.set_override(
        OWNER,
        rid,
        d("2024-01-01"),
        &OverrideCmd {
            amount_cents: 1000,
            description: None,
            update_rule: false,
        },
    )
    .await
    .unwrap();
    svc.set_override(
        OWNER,
        rid,
        d("2024-02-01"),
        &OverrideCmd {
            amount_cents: 2000,
            description: None,
            update_rule: false,
        },
    )
    .await
    .unwrap();

    let before = svc
        .occurrences(OWNER, rid, d("2024-01-01"), d("2024-04-30"))
        .await
        .unwrap();
    // Unconfirmed Mar/Apr estimate as the average of the last two real values.
    assert_eq!(by_date(&before, "2024-03-01").amount_cents, 1500);
    assert_eq!(by_date(&before, "2024-04-01").amount_cents, 1500);

    // Confirm March with a higher real value.
    svc.set_override(
        OWNER,
        rid,
        d("2024-03-01"),
        &OverrideCmd {
            amount_cents: 3000,
            description: None,
            update_rule: false,
        },
    )
    .await
    .unwrap();

    let after = svc
        .occurrences(OWNER, rid, d("2024-01-01"), d("2024-04-30"))
        .await
        .unwrap();
    // March is now confirmed (overridden); April's average shifts to (3000+2000)/2.
    let mar = by_date(&after, "2024-03-01");
    assert_eq!(mar.amount_cents, 3000);
    assert!(mar.overridden);
    assert_eq!(by_date(&after, "2024-04-01").amount_cents, 2500);
}

/// Gate 3 ("update_rule"): confirming a date with the flag re-bases the fixed
/// rule's amount, so occurrences generated AFTER the change reflect the new
/// amount, while occurrences already confirmed in the past keep their original
/// (confirmed) values. Without the flag the rule's amount is left untouched.
#[tokio::test]
async fn update_rule_rebases_future_occurrences() {
    let (svc, _store) = setup().await;

    // Rule B: confirm Jan with update_rule (re-bases the rule to 2000), then
    // confirm Feb without the flag with a *different* value (500).
    let b = svc
        .create(
            OWNER,
            OWNER,
            &fixed_cmd("FREQ=MONTHLY;BYMONTHDAY=15", 1000, "2024-01-15"),
        )
        .await
        .unwrap();
    svc.set_override(
        OWNER,
        &b.id,
        d("2024-01-15"),
        &OverrideCmd {
            amount_cents: 2000,
            description: None,
            update_rule: true,
        },
    )
    .await
    .unwrap();
    svc.set_override(
        OWNER,
        &b.id,
        d("2024-02-15"),
        &OverrideCmd {
            amount_cents: 500,
            description: None,
            update_rule: false,
        },
    )
    .await
    .unwrap();

    // The rule amount is re-based to the value that triggered update_rule.
    assert_eq!(svc.get(OWNER, &b.id).await.unwrap().amount_cents, 2000);

    let occs = svc
        .occurrences(OWNER, &b.id, d("2024-01-01"), d("2024-04-30"))
        .await
        .unwrap();

    // Past confirmed occurrences keep their ORIGINAL values: Feb stays 500,
    // not the re-based 2000.
    let jan = by_date(&occs, "2024-01-15");
    assert_eq!(jan.amount_cents, 2000);
    assert!(jan.overridden);
    let feb = by_date(&occs, "2024-02-15");
    assert_eq!(
        feb.amount_cents, 500,
        "a past confirmed value must not be overwritten by the re-based amount"
    );
    assert!(feb.overridden);

    // Future (unconfirmed) occurrences reflect the new (re-based) amount.
    let mar = by_date(&occs, "2024-03-15");
    assert_eq!(mar.amount_cents, 2000);
    assert!(!mar.overridden);
    let apr = by_date(&occs, "2024-04-15");
    assert_eq!(apr.amount_cents, 2000);
    assert!(!apr.overridden);

    // Contrast: without the flag the rule amount is untouched, so future
    // occurrences do NOT adopt the confirmed value.
    let a = svc
        .create(
            OWNER,
            OWNER,
            &fixed_cmd("FREQ=MONTHLY;BYMONTHDAY=15", 1000, "2024-01-15"),
        )
        .await
        .unwrap();
    svc.set_override(
        OWNER,
        &a.id,
        d("2024-01-15"),
        &OverrideCmd {
            amount_cents: 2000,
            description: None,
            update_rule: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(svc.get(OWNER, &a.id).await.unwrap().amount_cents, 1000);
    let a_occ = svc
        .occurrences(OWNER, &a.id, d("2024-01-01"), d("2024-03-31"))
        .await
        .unwrap();
    assert_eq!(by_date(&a_occ, "2024-01-15").amount_cents, 2000); // confirmed
    assert_eq!(by_date(&a_occ, "2024-02-15").amount_cents, 1000); // unchanged rule
    assert_eq!(by_date(&a_occ, "2024-03-15").amount_cents, 1000);
}

// ─── Re-aligning already-registered occurrences on rule update ────────────────

fn align_cmd(amount: i64) -> RecurrenceUpdateCmd {
    RecurrenceUpdateCmd {
        amount_cents: Some(amount),
        apply_to_existing: true,
        ..Default::default()
    }
}

/// Amounts of the source-leg transactions of `rec_id`, by date.
async fn tx_amounts(storage: &Storage, rec_id: &str) -> Vec<(NaiveDate, i64)> {
    SqlxTransactionsRepo::new(storage)
        .list_for_recurrence(rec_id, None)
        .await
        .unwrap()
        .into_iter()
        .filter(|t| !t.client_id.ends_with(":pair"))
        .map(|t| (t.date, t.cents))
        .collect()
}

/// Without `apply_to_existing` the registered occurrences keep the old amount
/// (only the rule changes).
#[tokio::test]
async fn update_without_flag_leaves_registered_occurrences() {
    let (svc, store) = setup().await;
    let rec = svc
        .create(
            OWNER,
            OWNER,
            &fixed_cmd("FREQ=MONTHLY;BYMONTHDAY=15", -1000, "2026-01-15"),
        )
        .await
        .unwrap();
    let cmd = RecurrenceUpdateCmd {
        amount_cents: Some(-1200),
        ..Default::default()
    };
    let (_, report) = svc.update_with_report(OWNER, &rec.id, &cmd).await.unwrap();
    assert_eq!((report.aligned, report.skipped_manual), (0, 0));
    let rows = tx_amounts(&store, &rec.id).await;
    assert!(!rows.is_empty());
    assert!(rows.iter().all(|(_, c)| *c == -1000));
}

/// Scenario 1: the amount was wrong from the start — every registered
/// occurrence is realigned.
#[tokio::test]
async fn apply_to_existing_realigns_all_rows() {
    let (svc, store) = setup().await;
    let rec = svc
        .create(
            OWNER,
            OWNER,
            &fixed_cmd("FREQ=MONTHLY;BYMONTHDAY=15", -1000, "2026-01-15"),
        )
        .await
        .unwrap();
    let n = tx_amounts(&store, &rec.id).await.len() as u64;
    assert!(n >= 2);

    let (updated, report) = svc
        .update_with_report(OWNER, &rec.id, &align_cmd(-1200))
        .await
        .unwrap();
    assert_eq!(updated.amount_cents, -1200);
    assert_eq!((report.aligned, report.skipped_manual), (n, 0));
    assert!(tx_amounts(&store, &rec.id)
        .await
        .iter()
        .all(|(_, c)| *c == -1200));
}

/// Scenario 2: price increase from a date — only rows on/after `apply_from`
/// change.
#[tokio::test]
async fn apply_from_limits_realignment() {
    let (svc, store) = setup().await;
    let rec = svc
        .create(
            OWNER,
            OWNER,
            &fixed_cmd("FREQ=MONTHLY;BYMONTHDAY=15", -1000, "2026-01-15"),
        )
        .await
        .unwrap();
    let cut = d("2026-06-15");
    let cmd = RecurrenceUpdateCmd {
        apply_from: Some(cut),
        ..align_cmd(-1300)
    };
    svc.update_with_report(OWNER, &rec.id, &cmd).await.unwrap();
    let rows = tx_amounts(&store, &rec.id).await;
    assert!(rows.iter().any(|(date, _)| *date < cut));
    assert!(rows.iter().any(|(date, _)| *date >= cut));
    for (date, cents) in rows {
        assert_eq!(cents, if date >= cut { -1300 } else { -1000 }, "{date}");
    }
}

/// Hand-modified rows are skipped by default and rewritten with
/// `overwrite_manual`; per-date overrides follow the same rule.
#[tokio::test]
async fn manual_rows_skipped_unless_overwrite() {
    let (svc, store) = setup().await;
    let rec = svc
        .create(
            OWNER,
            OWNER,
            &fixed_cmd("FREQ=MONTHLY;BYMONTHDAY=15", -1000, "2026-01-15"),
        )
        .await
        .unwrap();
    let repo = SqlxTransactionsRepo::new(&store);
    let rows = repo.list_for_recurrence(&rec.id, None).await.unwrap();
    let n = rows.len() as u64;

    // Edit one row by hand, and pin another date with a per-date override.
    let mut edited = rows[0].clone();
    edited.cents = -999;
    repo.update(&edited).await.unwrap();
    let pinned_date = rows[1].date;
    svc.set_override(
        OWNER,
        &rec.id,
        pinned_date,
        &OverrideCmd {
            amount_cents: -1000,
            description: None,
            update_rule: false,
        },
    )
    .await
    .unwrap();

    let (_, report) = svc
        .update_with_report(OWNER, &rec.id, &align_cmd(-1200))
        .await
        .unwrap();
    assert_eq!((report.aligned, report.skipped_manual), (n - 2, 2));
    let amounts = tx_amounts(&store, &rec.id).await;
    assert_eq!(amounts[0].1, -999);
    assert_eq!(amounts[1].1, -1000);
    assert!(amounts[2..].iter().all(|(_, c)| *c == -1200));

    let cmd = RecurrenceUpdateCmd {
        overwrite_manual: true,
        ..align_cmd(-1200)
    };
    let (_, report) = svc.update_with_report(OWNER, &rec.id, &cmd).await.unwrap();
    assert_eq!((report.aligned, report.skipped_manual), (2, 0));
    assert!(tx_amounts(&store, &rec.id)
        .await
        .iter()
        .all(|(_, c)| *c == -1200));
    // The pinned per-date override follows, so the occurrence view agrees.
    let occ = svc
        .occurrences(OWNER, &rec.id, pinned_date, pinned_date)
        .await
        .unwrap();
    assert_eq!(occ[0].amount_cents, -1200);
}

/// Both legs of a transfer recurrence are realigned, with opposite signs.
#[tokio::test]
async fn apply_to_existing_realigns_transfer_legs() {
    let (svc, store) = setup().await;
    sqlx::query(
        "INSERT INTO accounts (id, owner_id, name, kind, start_date, start_balance_cents, active, created_at)
         VALUES ('acc2', 'u1', 'risparmio', 'current', '2024-01-01', 0, 1, '2025-01-01T00:00:00Z')",
    )
    .execute(store.pool())
    .await
    .unwrap();
    let mut cmd = fixed_cmd("FREQ=MONTHLY;BYMONTHDAY=15", -1000, "2026-01-15");
    cmd.linked_account_id = Some("acc2".into());
    let rec = svc.create(OWNER, OWNER, &cmd).await.unwrap();

    svc.update_with_report(OWNER, &rec.id, &align_cmd(-1500))
        .await
        .unwrap();
    let legs = SqlxTransactionsRepo::new(&store)
        .list_for_recurrence(&rec.id, None)
        .await
        .unwrap();
    assert!(legs.len() >= 4);
    for leg in legs {
        let expected = if leg.client_id.ends_with(":pair") {
            1500
        } else {
            -1500
        };
        assert_eq!(leg.cents, expected, "{}", leg.client_id);
    }
}

/// Re-aligning past rows only makes sense for fixed-amount rules.
#[tokio::test]
async fn apply_to_existing_rejected_for_avg_rules() {
    let (svc, _store) = setup().await;
    let rec = svc
        .create(
            OWNER,
            OWNER,
            &avg_cmd("FREQ=MONTHLY;BYMONTHDAY=15", 3, "2026-01-15"),
        )
        .await
        .unwrap();
    let err = svc
        .update_with_report(OWNER, &rec.id, &align_cmd(-500))
        .await
        .unwrap_err();
    assert!(matches!(err, RecurrenceError::Validation(_)), "{err:?}");
}

// ─── Start-date changes: nothing happens unless the caller asks ───────────────

#[tokio::test]
async fn moving_start_earlier_creates_rows_only_on_request() {
    let (svc, store) = setup().await;
    let rec = svc
        .create(
            OWNER,
            OWNER,
            &fixed_cmd("FREQ=MONTHLY;BYMONTHDAY=15", -1000, "2026-01-15"),
        )
        .await
        .unwrap();
    let before = tx_amounts(&store, &rec.id).await.len();
    let earlier = Some(d("2025-10-15"));

    let impact = svc
        .date_impact(
            OWNER,
            &rec.id,
            &RecurrenceUpdateCmd {
                first_date: earlier,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!((impact.to_create, impact.stale_auto), (3, 0));

    let cmd = RecurrenceUpdateCmd {
        first_date: earlier,
        ..Default::default()
    };
    svc.update_with_report(OWNER, &rec.id, &cmd).await.unwrap();
    assert_eq!(tx_amounts(&store, &rec.id).await.len(), before);

    let cmd = RecurrenceUpdateCmd {
        first_date: earlier,
        create_missing: true,
        ..Default::default()
    };
    let (_, report) = svc.update_with_report(OWNER, &rec.id, &cmd).await.unwrap();
    assert_eq!(report.created, 3);
    assert_eq!(tx_amounts(&store, &rec.id).await.len(), before + 3);
}

#[tokio::test]
async fn moving_start_later_deletes_rows_only_on_request() {
    let (svc, store) = setup().await;
    let rec = svc
        .create(
            OWNER,
            OWNER,
            &fixed_cmd("FREQ=MONTHLY;BYMONTHDAY=15", -1000, "2026-01-15"),
        )
        .await
        .unwrap();
    let before = tx_amounts(&store, &rec.id).await.len();
    let later = Some(d("2026-03-15"));

    let impact = svc
        .date_impact(
            OWNER,
            &rec.id,
            &RecurrenceUpdateCmd {
                first_date: later,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        (impact.to_create, impact.stale_auto, impact.stale_manual),
        (0, 2, 0)
    );

    let cmd = RecurrenceUpdateCmd {
        first_date: later,
        ..Default::default()
    };
    svc.update_with_report(OWNER, &rec.id, &cmd).await.unwrap();
    assert_eq!(tx_amounts(&store, &rec.id).await.len(), before);

    let cmd = RecurrenceUpdateCmd {
        first_date: later,
        stale_action: pfm_service::StaleAction::DeleteAuto,
        ..Default::default()
    };
    let (_, report) = svc.update_with_report(OWNER, &rec.id, &cmd).await.unwrap();
    assert_eq!(report.removed, 2);
    assert_eq!(tx_amounts(&store, &rec.id).await.len(), before - 2);
}

// ─── Holiday calendars as data ──────────────────────────────────────────────

/// Salary-like rule: the 27th of each month, moved to the previous business day.
fn friday_27(first: &str) -> RecurrenceCreateCmd {
    RecurrenceCreateCmd {
        weekend: WeekendPolicy::Friday,
        ..fixed_cmd("FREQ=MONTHLY;BYMONTHDAY=27", -1000, first)
    }
}

fn rule(label: &str, kind: &str) -> pfm_domain::RuleParts {
    pfm_domain::RuleParts {
        label: label.into(),
        kind: kind.into(),
        ..Default::default()
    }
}

async fn december(svc: &RecurrenceService, id: &str) -> Vec<NaiveDate> {
    svc.occurrences(OWNER, id, d("2026-12-01"), d("2026-12-31"))
        .await
        .unwrap()
        .into_iter()
        .map(|o| o.date)
        .collect()
}

#[tokio::test]
async fn recurrence_dates_follow_the_users_calendar_and_its_edits() {
    use pfm_service::HolidayService;
    let (svc, storage) = setup().await;
    let hol = HolidayService::new(&storage);
    let rec = svc
        .create(OWNER, OWNER, &friday_27("2026-12-01"))
        .await
        .unwrap();
    let id = rec.id.clone();

    // Seeded Italian calendar (the default): Sun 27 → Fri 25 is Christmas → Thu 24.
    assert_eq!(december(&svc, &id).await, vec![d("2026-12-24")]);

    // An admin removes Christmas from the calendar: the rule lands on Fri 25.
    let mut rules = hol.rules("it").await.unwrap();
    rules.retain(|r| r.label != "Natale");
    hol.replace_rules("it", &rules).await.unwrap();
    assert_eq!(december(&svc, &id).await, vec![d("2026-12-25")]);

    // …and adds a one-off closure on Fri 25 Dec: back to Thu 24.
    rules.push(pfm_domain::RuleParts {
        date: Some("2026-12-25".into()),
        ..rule("Chiusura", "date")
    });
    hol.replace_rules("it", &rules).await.unwrap();
    assert_eq!(december(&svc, &id).await, vec![d("2026-12-24")]);
}

#[tokio::test]
async fn calendar_admin_validation_and_lifecycle() {
    use pfm_service::{HolidayError, HolidayService};
    let (_svc, storage) = setup().await;
    let hol = HolidayService::new(&storage);

    // Seeded calendars exist, with rules.
    let codes: Vec<String> = hol
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.code)
        .collect();
    assert!(["none", "it", "de", "fr", "es", "gb", "us"]
        .iter()
        .all(|c| codes.contains(&c.to_string())));
    let preview = HolidayService::preview(&hol.rules("it").await.unwrap(), 2026).unwrap();
    assert!(preview.iter().any(|(d, _)| *d == d_("2026-04-06"))); // Pasquetta
    assert!(preview.iter().any(|(d, _)| *d == d_("2026-12-25")));

    // An invalid rule list is rejected as a whole and changes nothing.
    let before = hol.rules("it").await.unwrap();
    let bad = vec![
        rule("ok", "easter"),
        pfm_domain::RuleParts {
            month: Some(4),
            day: Some(31),
            ..rule("bad", "fixed")
        },
    ];
    assert!(matches!(
        hol.replace_rules("it", &bad).await,
        Err(HolidayError::Validation(_))
    ));
    assert_eq!(hol.rules("it").await.unwrap(), before);

    // Create / duplicate / rename / delete; deleting a calendar in use is refused.
    hol.create("test-cal", "Test").await.unwrap();
    assert!(matches!(
        hol.create("test-cal", "Again").await,
        Err(HolidayError::Duplicate)
    ));
    assert!(matches!(
        hol.create("Bad Code", "x").await,
        Err(HolidayError::Validation(_))
    ));
    hol.rename("test-cal", "Renamed").await.unwrap();
    hol.delete("test-cal").await.unwrap();
    assert!(matches!(
        hol.delete("test-cal").await,
        Err(HolidayError::NotFound)
    ));
    sqlx::query("INSERT INTO user_prefs (user_id, holiday_calendar) VALUES ('u1', 'de') ON CONFLICT(user_id) DO UPDATE SET holiday_calendar = 'de'")
        .execute(storage.pool()).await.unwrap();
    assert!(matches!(
        hol.delete("de").await,
        Err(HolidayError::InUse(1))
    ));
}

fn d_(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

/// A transfer whose source leg was deleted (only the linked leg survives) is
/// reported as missing and recreated on request — without duplicating the leg
/// that is still there.
#[tokio::test]
async fn transfer_with_one_deleted_leg_is_detected_and_recreated() {
    let (svc, store) = setup().await;
    sqlx::query(
        "INSERT INTO accounts (id, owner_id, name, kind, start_date, start_balance_cents, active, created_at)
         VALUES ('acc2', 'u1', 'risparmio', 'current', '2024-01-01', 0, 1, '2025-01-01T00:00:00Z')",
    )
    .execute(store.pool())
    .await
    .unwrap();
    let mut cmd = fixed_cmd("FREQ=MONTHLY;BYMONTHDAY=15", -1000, "2026-01-15");
    cmd.linked_account_id = Some("acc2".into());
    let rec = svc.create(OWNER, OWNER, &cmd).await.unwrap();

    let repo = SqlxTransactionsRepo::new(&store);
    let legs = repo.list_for_recurrence(&rec.id, None).await.unwrap();
    let total = legs.len();
    // Delete the source leg of every occurrence.
    let sources: Vec<_> = legs
        .iter()
        .filter(|t| !t.client_id.ends_with(":pair"))
        .collect();
    for t in &sources {
        repo.delete(&t.id).await.unwrap();
    }
    let n = sources.len() as u64;
    assert!(n >= 2);

    let impact = svc
        .date_impact(OWNER, &rec.id, &RecurrenceUpdateCmd::default())
        .await
        .unwrap();
    assert_eq!((impact.to_create, impact.stale_auto), (n, 0));

    let cmd = RecurrenceUpdateCmd {
        create_missing: true,
        ..Default::default()
    };
    svc.update_with_report(OWNER, &rec.id, &cmd).await.unwrap();
    assert_eq!(
        repo.list_for_recurrence(&rec.id, None).await.unwrap().len(),
        total
    );
    let again = svc
        .date_impact(OWNER, &rec.id, &RecurrenceUpdateCmd::default())
        .await
        .unwrap();
    assert_eq!(again.to_create, 0);
}

/// Materialized occurrences of a transfer recurrence show up in the transfers list.
#[tokio::test]
async fn transfer_recurrence_rows_are_listed_as_transfers() {
    let (svc, store) = setup().await;
    sqlx::query(
        "INSERT INTO accounts (id, owner_id, name, kind, start_date, start_balance_cents, active, created_at)
         VALUES ('acc2', 'u1', 'risparmio', 'current', '2024-01-01', 0, 1, '2025-01-01T00:00:00Z')",
    )
    .execute(store.pool())
    .await
    .unwrap();
    let mut cmd = fixed_cmd("FREQ=MONTHLY;BYMONTHDAY=15", -1000, "2026-01-15");
    cmd.linked_account_id = Some("acc2".into());
    svc.create(OWNER, OWNER, &cmd).await.unwrap();

    let transfers = pfm_service::TransferService::new(&store)
        .list(OWNER, d("2026-01-01"), d("2026-12-31"), None, None)
        .await
        .unwrap();
    assert!(transfers.len() >= 2, "got {}", transfers.len());
    assert!(transfers
        .iter()
        .all(|t| t.from_account_id == ACCOUNT && t.to_account_id == "acc2" && t.cents == 1000));
}

/// Set (or clear) the date up to which the scheduler registered `rec_id`.
async fn set_mark(store: &Storage, rec_id: &str, mark: Option<&str>) {
    sqlx::query("UPDATE recurrences SET materialized_through = $2 WHERE id = $1")
        .bind(rec_id)
        .bind(mark.map(str::to_string))
        .execute(store.pool())
        .await
        .unwrap();
}

/// An occurrence that falls due is registered by the periodic pass — once —
/// instead of dropping out of the forecast without reaching the ledger.
#[tokio::test]
async fn due_occurrences_are_registered_by_the_periodic_pass() {
    let (svc, store) = setup().await;
    let rec = svc
        .create(
            OWNER,
            OWNER,
            &fixed_cmd("FREQ=MONTHLY;BYMONTHDAY=27", -1000, "2030-01-27"),
        )
        .await
        .unwrap();
    assert!(tx_amounts(&store, &rec.id).await.is_empty());
    set_mark(&store, &rec.id, Some("2030-01-26")).await;

    assert_eq!(svc.materialize_all_due_at(d("2030-01-27")).await, 1);
    assert_eq!(svc.materialize_all_due_at(d("2030-01-27")).await, 0);
    assert_eq!(svc.materialize_all_due_at(d("2030-03-01")).await, 1);
    assert_eq!(
        tx_amounts(&store, &rec.id).await,
        vec![(d("2030-01-27"), -1000), (d("2030-02-27"), -1000)]
    );
}

/// The pass only covers the dates after its previous run: a registered row the
/// user deleted on purpose is not brought back.
#[tokio::test]
async fn periodic_pass_does_not_recreate_deleted_rows() {
    let (svc, store) = setup().await;
    let rec = svc
        .create(
            OWNER,
            OWNER,
            &fixed_cmd("FREQ=MONTHLY;BYMONTHDAY=27", -1000, "2030-01-27"),
        )
        .await
        .unwrap();
    set_mark(&store, &rec.id, Some("2030-01-26")).await;
    svc.materialize_all_due_at(d("2030-02-27")).await;
    let repo = SqlxTransactionsRepo::new(&store);
    let jan = repo
        .list_for_recurrence(&rec.id, None)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.date == d("2030-01-27"))
        .unwrap();
    repo.delete(&jan.id).await.unwrap();

    assert_eq!(svc.materialize_all_due_at(d("2030-03-27")).await, 1);
    assert_eq!(
        tx_amounts(&store, &rec.id).await,
        vec![(d("2030-02-27"), -1000), (d("2030-03-27"), -1000)]
    );
}

/// A Saturday occurrence moved back to Friday by the weekend policy is due on
/// the Friday, even though its nominal date is after "today".
#[tokio::test]
async fn periodic_pass_registers_occurrences_shifted_into_the_window() {
    let (svc, store) = setup().await;
    let mut cmd = fixed_cmd("FREQ=MONTHLY;BYMONTHDAY=27", -1000, "2030-01-27");
    cmd.weekend = WeekendPolicy::Friday;
    let rec = svc.create(OWNER, OWNER, &cmd).await.unwrap();
    // 2030-04-27 is a Saturday.
    set_mark(&store, &rec.id, Some("2030-04-25")).await;

    assert_eq!(svc.materialize_all_due_at(d("2030-04-26")).await, 1);
    assert_eq!(
        tx_amounts(&store, &rec.id).await,
        vec![(d("2030-04-26"), -1000)]
    );
}

/// Never run on a rule (mark NULL): the pass starts after its latest row, and
/// does not re-fill older gaps.
#[tokio::test]
async fn periodic_pass_without_mark_starts_after_latest_row() {
    let (svc, store) = setup().await;
    let rec = svc
        .create(
            OWNER,
            OWNER,
            &fixed_cmd("FREQ=MONTHLY;BYMONTHDAY=27", -1000, "2030-01-27"),
        )
        .await
        .unwrap();
    set_mark(&store, &rec.id, Some("2030-01-26")).await;
    svc.materialize_all_due_at(d("2030-02-27")).await;
    let repo = SqlxTransactionsRepo::new(&store);
    let jan = repo
        .list_for_recurrence(&rec.id, None)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.date == d("2030-01-27"))
        .unwrap();
    repo.delete(&jan.id).await.unwrap();
    set_mark(&store, &rec.id, None).await;

    assert_eq!(svc.materialize_all_due_at(d("2030-04-27")).await, 2);
    assert_eq!(
        tx_amounts(&store, &rec.id)
            .await
            .into_iter()
            .map(|(d, _)| d)
            .collect::<Vec<_>>(),
        vec![d("2030-02-27"), d("2030-03-27"), d("2030-04-27")]
    );
}

/// While a rule is inactive nothing is registered and the mark still advances:
/// reactivating it does not back-fill the pause.
#[tokio::test]
async fn periodic_pass_skips_inactive_rules_without_back_fill() {
    let (svc, store) = setup().await;
    let rec = svc
        .create(
            OWNER,
            OWNER,
            &fixed_cmd("FREQ=MONTHLY;BYMONTHDAY=27", -1000, "2030-01-27"),
        )
        .await
        .unwrap();
    set_mark(&store, &rec.id, Some("2030-01-26")).await;
    let off = RecurrenceUpdateCmd {
        active: Some(false),
        ..Default::default()
    };
    svc.update(OWNER, &rec.id, &off).await.unwrap();
    assert_eq!(svc.materialize_all_due_at(d("2030-03-01")).await, 0);

    let on = RecurrenceUpdateCmd {
        active: Some(true),
        ..Default::default()
    };
    svc.update(OWNER, &rec.id, &on).await.unwrap();
    assert_eq!(svc.materialize_all_due_at(d("2030-03-27")).await, 1);
    assert_eq!(
        tx_amounts(&store, &rec.id).await,
        vec![(d("2030-03-27"), -1000)]
    );
}

/// A value confirmed in advance for a date is the one registered when it falls
/// due, as the forecast showed it.
#[tokio::test]
async fn periodic_pass_registers_the_confirmed_amount() {
    let (svc, store) = setup().await;
    let rec = svc
        .create(
            OWNER,
            OWNER,
            &fixed_cmd("FREQ=MONTHLY;BYMONTHDAY=27", -1000, "2030-01-27"),
        )
        .await
        .unwrap();
    set_mark(&store, &rec.id, Some("2030-01-26")).await;
    svc.set_override(
        OWNER,
        &rec.id,
        d("2030-01-27"),
        &OverrideCmd {
            amount_cents: -1234,
            description: None,
            update_rule: false,
        },
    )
    .await
    .unwrap();

    svc.materialize_all_due_at(d("2030-01-27")).await;
    assert_eq!(
        tx_amounts(&store, &rec.id).await,
        vec![(d("2030-01-27"), -1234)]
    );
}
