//! Recurrence use-cases: rule CRUD (permission-checked on the rule's account),
//! runtime occurrence generation (never materialized in the database), and
//! per-date override confirmation — including the "also update the rule" flag
//! that re-bases a fixed rule's amount so future occurrences start from the
//! confirmed value.
//!
//! Occurrences are computed on the fly by driving the pure
//! [`generate_occurrences`] engine with the rule's pattern plus the confirmed
//! real occurrences (overridden transactions + the `recurrence_overrides`
//! table). Dates that already carry a confirmed value are never re-generated:
//! the engine output for those dates is replaced by the confirmed value before
//! the response is returned.

use std::collections::{BTreeMap, BTreeSet};

use chrono::NaiveDate;
use pfm_domain::{
    can_see, can_write, generate_occurrences_in, Account, CategoryKind, EndType, PatternParts,
    RealOccurrence, Recurrence, RecurrenceMode, RecurrenceOccurrence, RecurrenceOverride,
    RecurrenceSpec, Role, Transaction, WeekendPolicy,
};
use pfm_repo::{
    AccountsRepo, OverridesRepo, RecurrencesRepo, SharesRepo, SqlxAccountsRepo, SqlxAuthRepo,
    SqlxOverridesRepo, SqlxRecurrencesRepo, SqlxSharesRepo, SqlxTransactionsRepo, Storage,
    TransactionsRepo, UsersRepo,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RecurrenceError {
    #[error("recurrence not found")]
    NotFound,
    #[error("account not found")]
    AccountNotFound,
    #[error("forbidden")]
    Forbidden,
    #[error("validation error: {0}")]
    Validation(String),
    #[error("internal error: {0}")]
    Internal(String),
}

/// Input for creating a new recurrence rule.
pub struct RecurrenceCreateCmd {
    pub description: String,
    pub category_id: String,
    pub kind: CategoryKind,
    pub amount_cents: i64,
    pub account_id: String,
    pub pattern: String,
    pub weekend: WeekendPolicy,
    pub first_date: NaiveDate,
    pub end_type: EndType,
    pub end_date: Option<NaiveDate>,
    pub end_count: Option<i64>,
    pub mode: RecurrenceMode,
    pub avg_n: i64,
    pub active: bool,
    pub note: String,
    pub linked_account_id: Option<String>,
    pub linked_category_id: Option<String>,
}

/// Input for updating an existing rule. All fields are `Option`; `None` means
/// "keep the current value". The nullable end fields use a double `Option` so a
/// caller can distinguish "leave unchanged" (`None`) from "clear" (`Some(None)`).
#[derive(Default)]
pub struct RecurrenceUpdateCmd {
    /// Also rewrite the amount of the transactions already materialized for
    /// this rule (fixed-amount rules only) — see [`AlignReport`].
    pub apply_to_existing: bool,
    /// Lower bound (inclusive) of the transactions rewritten when
    /// `apply_to_existing` is set; `None` means every occurrence of the rule.
    pub apply_from: Option<NaiveDate>,
    /// When set together with `apply_to_existing`, also rewrite the
    /// occurrences that were modified by hand (amount different from the
    /// rule's previous one, flagged `override`, or pinned by a per-date
    /// override). Otherwise those are left untouched.
    pub overwrite_manual: bool,
    /// After a start-date/pattern change: register the past occurrences the new
    /// schedule generates that are not in the ledger yet.
    pub create_missing: bool,
    /// After a start-date/pattern change: what to do with registered rows the
    /// new schedule no longer generates.
    pub stale_action: StaleAction,
    pub description: Option<String>,
    pub category_id: Option<String>,
    pub amount_cents: Option<i64>,
    pub account_id: Option<String>,
    pub pattern: Option<String>,
    pub weekend: Option<WeekendPolicy>,
    pub first_date: Option<NaiveDate>,
    pub end_type: Option<EndType>,
    pub end_date: Option<Option<NaiveDate>>,
    pub end_count: Option<Option<i64>>,
    pub mode: Option<RecurrenceMode>,
    pub avg_n: Option<i64>,
    pub active: Option<bool>,
    pub note: Option<String>,
    /// `Some(None)` clears the linked account (removes transfer behaviour).
    pub linked_account_id: Option<Option<String>>,
    /// `Some(None)` clears the linked category.
    pub linked_category_id: Option<Option<String>>,
}

/// What to do with registered rows that a changed schedule no longer generates.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum StaleAction {
    /// Leave them in the ledger.
    #[default]
    Keep,
    /// Delete them, except those modified by hand.
    DeleteAuto,
    /// Delete them all, modified ones included.
    DeleteAll,
}

/// Impact of a schedule change on the registered rows (occurrence counts).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DateImpact {
    /// Past occurrences of the new schedule not yet in the ledger.
    pub to_create: u64,
    /// Registered rows no longer generated, untouched by hand.
    pub stale_auto: u64,
    /// Registered rows no longer generated, modified by hand.
    pub stale_manual: u64,
}

/// Outcome of re-aligning already-registered occurrences after a rule update
/// (all zero when `apply_to_existing` was not requested).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AlignReport {
    /// Transactions whose amount was rewritten.
    pub aligned: u64,
    /// Transactions left untouched because they were modified by hand and
    /// `overwrite_manual` was not set.
    pub skipped_manual: u64,
    /// Registered rows deleted because the new schedule no longer generates them.
    pub removed: u64,
    /// Past occurrences registered because the schedule was moved earlier.
    pub created: u64,
}

/// Input for confirming (overriding) a single occurrence of a rule.
pub struct OverrideCmd {
    /// The confirmed real amount in cents (signed).
    pub amount_cents: i64,
    /// Optional description for this occurrence.
    pub description: Option<String>,
    /// When `true` and the rule is `fixed`, the rule's `amount_cents` is also
    /// set to this value so future occurrences start from the confirmed amount.
    pub update_rule: bool,
}

pub struct RecurrenceService {
    recurrences: SqlxRecurrencesRepo,
    overrides: SqlxOverridesRepo,
    transactions: SqlxTransactionsRepo,
    accounts: SqlxAccountsRepo,
    users: SqlxAuthRepo,
    shares: SqlxSharesRepo,
    holidays: crate::holidays::HolidayService,
    category_access: crate::CategoryAccess,
}

impl RecurrenceService {
    pub fn new(storage: &Storage) -> Self {
        Self {
            recurrences: SqlxRecurrencesRepo::new(storage),
            overrides: SqlxOverridesRepo::new(storage),
            transactions: SqlxTransactionsRepo::new(storage),
            accounts: SqlxAccountsRepo::new(storage),
            users: SqlxAuthRepo::new(storage),
            holidays: crate::holidays::HolidayService::new(storage),
            shares: SqlxSharesRepo::new(storage),
            category_access: crate::CategoryAccess::new(storage),
        }
    }

    /// A `category_id` sent by a client must exist and be one the actor may use
    /// (own, shared, or a collaborator's); anything else is a validation error
    /// instead of a database foreign-key failure or a cross-user reference.
    async fn ensure_category_usable(
        &self,
        actor_id: &str,
        category_id: &str,
    ) -> Result<(), RecurrenceError> {
        self.category_access
            .ensure_usable(actor_id, category_id)
            .await
            .map_err(|e| match e {
                crate::CategoryAccessError::NotFound => {
                    RecurrenceError::Validation("unknown category".into())
                }
                crate::CategoryAccessError::Internal(m) => RecurrenceError::Internal(m),
            })
    }

    async fn check_access(
        &self,
        actor_id: &str,
        account: &Account,
        write: bool,
    ) -> Result<bool, RecurrenceError> {
        if account.owner_id == actor_id {
            return Ok(true);
        }

        let actor = self
            .users
            .get_by_id(actor_id)
            .await
            .map_err(|e| RecurrenceError::Internal(e.to_string()))?
            .ok_or_else(|| RecurrenceError::Internal("actor not found".into()))?;

        if actor.role == Role::Admin {
            return Ok(true);
        }

        let share = self
            .shares
            .get(&account.id, actor_id)
            .await
            .map_err(|e| RecurrenceError::Internal(e.to_string()))?;

        if !write {
            Ok(can_see(
                actor.role,
                actor_id,
                &account.owner_id,
                share.is_some(),
            ))
        } else {
            Ok(can_write(
                actor.role,
                actor_id,
                &account.owner_id,
                share.map(|s| s.level),
            ))
        }
    }

    /// Load the account for `account_id` and enforce access for `actor`.
    async fn load_account_checked(
        &self,
        actor_id: &str,
        account_id: &str,
        write: bool,
    ) -> Result<(), RecurrenceError> {
        let account = self
            .accounts
            .get_by_id(account_id)
            .await
            .map_err(|e| RecurrenceError::Internal(e.to_string()))?
            .ok_or(RecurrenceError::AccountNotFound)?;

        if !self.check_access(actor_id, &account, write).await? {
            return Err(RecurrenceError::Forbidden);
        }

        Ok(())
    }

    /// Whether `actor` may read the account at `account_id` (a missing account
    /// is simply not visible).
    async fn may_see_account(
        &self,
        actor_id: &str,
        account_id: &str,
    ) -> Result<bool, RecurrenceError> {
        let account = match self
            .accounts
            .get_by_id(account_id)
            .await
            .map_err(|e| RecurrenceError::Internal(e.to_string()))?
        {
            Some(a) => a,
            None => return Ok(false),
        };
        self.check_access(actor_id, &account, false).await
    }

    fn validate_end_fields(
        end_type: EndType,
        end_date: Option<NaiveDate>,
        end_count: Option<i64>,
    ) -> Result<(), RecurrenceError> {
        match end_type {
            EndType::Never => {
                if end_date.is_some() || end_count.is_some() {
                    return Err(RecurrenceError::Validation(
                        "end_date / end_count must be null when end_type=never".into(),
                    ));
                }
            }
            EndType::Until => {
                if end_date.is_none() {
                    return Err(RecurrenceError::Validation(
                        "end_date is required when end_type=until".into(),
                    ));
                }
            }
            EndType::Count => {
                let n = end_count.ok_or_else(|| {
                    RecurrenceError::Validation("end_count is required when end_type=count".into())
                })?;
                if n < 1 {
                    return Err(RecurrenceError::Validation("end_count must be >= 1".into()));
                }
            }
        }
        Ok(())
    }

    /// Build the engine spec from a stored rule. The rule's structural pattern
    /// is parsed into frequency/interval/by_*; end and amount come from the
    /// stored columns.
    pub fn build_spec(
        rec: &Recurrence,
        real: Vec<RealOccurrence>,
    ) -> Result<RecurrenceSpec, RecurrenceError> {
        let parts = PatternParts::from_rrule(&rec.pattern)
            .map_err(|e| RecurrenceError::Validation(format!("invalid pattern: {e}")))?;

        Ok(RecurrenceSpec {
            frequency: parts.frequency,
            interval: parts.interval,
            by_monthday: parts.by_monthday,
            by_weekday: parts.by_weekday,
            weekend_policy: rec.weekend,
            first_date: rec.first_date,
            end: rec.end_spec(),
            amount: rec.amount_mode(real),
        })
    }

    /// The confirmed (date -> amount) values for a rule: overridden transactions
    /// first, then persisted overrides (which win on a date collision).
    async fn confirmed_map(
        &self,
        rec_id: &str,
    ) -> Result<BTreeMap<NaiveDate, i64>, RecurrenceError> {
        let mut map: BTreeMap<NaiveDate, i64> = BTreeMap::new();
        let real = self
            .transactions
            .real_occurrences_for_recurrence(rec_id)
            .await
            .map_err(|e| RecurrenceError::Internal(e.to_string()))?;
        for r in real {
            map.insert(r.date, r.amount_cents);
        }
        let ovs = self
            .overrides
            .list_for_recurrence(rec_id)
            .await
            .map_err(|e| RecurrenceError::Internal(e.to_string()))?;
        for o in ovs {
            map.insert(o.date, o.amount_cents);
        }
        Ok(map)
    }

    pub async fn create(
        &self,
        actor_id: &str,
        owner_id: &str,
        cmd: &RecurrenceCreateCmd,
    ) -> Result<Recurrence, RecurrenceError> {
        // The rule's account must exist and the actor must be able to write to it.
        self.load_account_checked(actor_id, &cmd.account_id, true)
            .await?;
        // A transfer rule also posts a leg on the linked account.
        if let Some(linked) = cmd.linked_account_id.as_deref() {
            self.load_account_checked(actor_id, linked, true).await?;
        }
        self.ensure_category_usable(actor_id, &cmd.category_id)
            .await?;
        if let Some(cat) = cmd.linked_category_id.as_deref() {
            self.ensure_category_usable(actor_id, cat).await?;
        }

        Self::validate_end_fields(cmd.end_type, cmd.end_date, cmd.end_count)?;

        let rec = Recurrence {
            id: String::new(),
            owner_id: owner_id.into(),
            description: cmd.description.clone(),
            category_id: cmd.category_id.clone(),
            kind: cmd.kind,
            amount_cents: cmd.amount_cents,
            account_id: cmd.account_id.clone(),
            pattern: cmd.pattern.clone(),
            weekend: cmd.weekend,
            first_date: cmd.first_date,
            end_type: cmd.end_type,
            end_date: cmd.end_date,
            end_count: cmd.end_count,
            mode: cmd.mode,
            avg_n: cmd.avg_n,
            active: cmd.active,
            note: cmd.note.clone(),
            linked_account_id: cmd.linked_account_id.clone(),
            linked_category_id: cmd.linked_category_id.clone(),
            created_at: String::new(),
            updated_at: String::new(),
        };

        let rec = self
            .recurrences
            .create(&rec)
            .await
            .map_err(|e| RecurrenceError::Internal(e.to_string()))?;

        self.materialize_past_occurrences(&rec, actor_id).await?;

        Ok(rec)
    }

    /// Create real DB transactions for every past occurrence of `rec` (from
    /// `first_date` up to and including today), then mark the rule as
    /// registered through today so the periodic pass picks up from there.
    async fn materialize_past_occurrences(
        &self,
        rec: &Recurrence,
        actor_id: &str,
    ) -> Result<u64, RecurrenceError> {
        let today = chrono::Utc::now().date_naive();
        let created = self
            .materialize_range(rec, actor_id, Self::schedule_from(rec), today)
            .await?;
        self.recurrences
            .set_materialized_through(&rec.id, today)
            .await
            .map_err(|e| RecurrenceError::Internal(e.to_string()))?;
        Ok(created)
    }

    /// Register the occurrences of every rule that fell due since the previous
    /// pass. Run by the scheduler: without it an occurrence would vanish from
    /// the forecast (which only shows dates after today) on its due date
    /// without ever reaching the ledger. Returns the number of occurrences
    /// registered; a failing rule is logged and skipped.
    pub async fn materialize_all_due(&self) -> u64 {
        self.materialize_all_due_at(chrono::Utc::now().date_naive())
            .await
    }

    /// [`materialize_all_due`](Self::materialize_all_due) with an explicit
    /// "today".
    pub async fn materialize_all_due_at(&self, today: NaiveDate) -> u64 {
        let rules = match self.recurrences.list_all_with_watermark().await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("scheduler: failed to list recurrences: {e}");
                return 0;
            }
        };
        let mut total = 0;
        for (rec, mark) in rules {
            match self.materialize_due(&rec, mark, today).await {
                Ok(n) => total += n,
                Err(e) => tracing::warn!(
                    rec_id = %rec.id,
                    "scheduler: recurrence materialization failed: {e}"
                ),
            }
        }
        total
    }

    /// Register the occurrences of `rec` dated in `(mark, today]` and move the
    /// mark to today. Only the window after the mark is generated, so rows the
    /// user deleted on purpose are not recreated. The mark also advances while
    /// the rule is inactive: reactivating it does not back-fill the pause.
    async fn materialize_due(
        &self,
        rec: &Recurrence,
        mark: Option<NaiveDate>,
        today: NaiveDate,
    ) -> Result<u64, RecurrenceError> {
        if mark == Some(today) {
            return Ok(0);
        }
        let mark = match mark {
            Some(d) => d,
            // Never run: start after the latest registered row.
            None => self
                .transactions
                .list_for_recurrence(&rec.id, None)
                .await
                .map_err(|e| RecurrenceError::Internal(e.to_string()))?
                .iter()
                .map(|t| t.date)
                .max()
                .map_or(today - chrono::Duration::days(1), |d| d.min(today)),
        };
        let created = if rec.active && mark < today {
            let from = (mark + chrono::Duration::days(1)).max(Self::schedule_from(rec));
            self.materialize_range(rec, &rec.owner_id, from, today)
                .await?
        } else {
            0
        };
        self.recurrences
            .set_materialized_through(&rec.id, today)
            .await
            .map_err(|e| RecurrenceError::Internal(e.to_string()))?;
        Ok(created)
    }

    /// Create real DB transactions for the occurrences of `rec` dated in
    /// `[from, to]`. A date that already has its row is skipped, whether found
    /// by the stable `client_id` key or by a row of the rule on that date, so
    /// repeated calls are idempotent. Amounts follow the forecast: a confirmed
    /// value wins on its date, and average rules average the confirmed ones.
    async fn materialize_range(
        &self,
        rec: &Recurrence,
        actor_id: &str,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<u64, RecurrenceError> {
        if from > to {
            return Ok(0);
        }

        let confirmed = self.confirmed_map(&rec.id).await?;
        let real = confirmed
            .iter()
            .map(|(d, a)| RealOccurrence {
                date: *d,
                amount_cents: *a,
            })
            .collect();
        let spec = Self::build_spec(rec, real)?;
        let calendar = self.holidays.for_user(&rec.owner_id).await;
        let occurrences = generate_occurrences_in(&spec, from, to, &calendar);
        if occurrences.is_empty() {
            return Ok(0);
        }

        let (mut src_dates, mut lnk_dates): (BTreeSet<NaiveDate>, BTreeSet<NaiveDate>) =
            Default::default();
        for t in self
            .transactions
            .list_for_recurrence(&rec.id, None)
            .await
            .map_err(|e| RecurrenceError::Internal(e.to_string()))?
        {
            if t.client_id.ends_with(":pair") {
                lnk_dates.insert(t.date);
            } else {
                src_dates.insert(t.date);
            }
        }
        let mut created: u64 = 0;

        for mut occ in occurrences {
            if let Some(amount) = confirmed.get(&occ.date) {
                occ.amount_cents = *amount;
            }
            let src_client_id = format!("rec:{}:{}", rec.id, occ.date);

            if let Some(linked_acc) = &rec.linked_account_id {
                // Transfer recurrence: create two paired legs.
                let lnk_client_id = format!("rec:{}:{}:pair", rec.id, occ.date);
                let src_exists = self
                    .transactions
                    .find_by_client_id(&src_client_id)
                    .await
                    .map_err(|e| RecurrenceError::Internal(e.to_string()))?
                    .is_some();
                let lnk_exists = self
                    .transactions
                    .find_by_client_id(&lnk_client_id)
                    .await
                    .map_err(|e| RecurrenceError::Internal(e.to_string()))?
                    .is_some();
                let src_exists = src_exists || src_dates.contains(&occ.date);
                let lnk_exists = lnk_exists || lnk_dates.contains(&occ.date);
                if src_exists && lnk_exists {
                    continue;
                }
                // Deterministic pair_id so both legs agree even if created separately.
                let pair_id = format!("rec-pair:{}:{}", rec.id, occ.date);
                let linked_cat = rec
                    .linked_category_id
                    .as_deref()
                    .unwrap_or(&rec.category_id);
                if !src_exists {
                    let src_tx = Transaction {
                        id: String::new(),
                        client_id: src_client_id,
                        account_id: rec.account_id.clone(),
                        date: occ.date,
                        cents: occ.amount_cents,
                        description: Some(rec.description.clone()),
                        category_id: Some(rec.category_id.clone()),
                        note: None,
                        pair_id: Some(pair_id.clone()),
                        rec_id: Some(rec.id.clone()),
                        rule_id: None,
                        override_: None,
                        computed: None,
                        vacct_id: None,
                        import_hash: None,
                        created_by: actor_id.to_string(),
                        created_at: String::new(),
                        updated_at: String::new(),
                    };
                    self.transactions
                        .create(&src_tx)
                        .await
                        .map_err(|e| RecurrenceError::Internal(e.to_string()))?;
                }
                if !lnk_exists {
                    let lnk_tx = Transaction {
                        id: String::new(),
                        client_id: lnk_client_id,
                        account_id: linked_acc.clone(),
                        date: occ.date,
                        cents: -occ.amount_cents,
                        description: Some(rec.description.clone()),
                        category_id: Some(linked_cat.to_string()),
                        note: None,
                        pair_id: Some(pair_id),
                        rec_id: Some(rec.id.clone()),
                        rule_id: None,
                        override_: None,
                        computed: None,
                        vacct_id: None,
                        import_hash: None,
                        created_by: actor_id.to_string(),
                        created_at: String::new(),
                        updated_at: String::new(),
                    };
                    self.transactions
                        .create(&lnk_tx)
                        .await
                        .map_err(|e| RecurrenceError::Internal(e.to_string()))?;
                }
                created += 1;
            } else {
                // Standard (non-transfer) recurrence: single transaction.
                let already_exists = src_dates.contains(&occ.date)
                    || lnk_dates.contains(&occ.date)
                    || self
                        .transactions
                        .find_by_client_id(&src_client_id)
                        .await
                        .map_err(|e| RecurrenceError::Internal(e.to_string()))?
                        .is_some();
                if already_exists {
                    continue;
                }
                let tx = Transaction {
                    id: String::new(),
                    client_id: src_client_id,
                    account_id: rec.account_id.clone(),
                    date: occ.date,
                    cents: occ.amount_cents,
                    description: Some(rec.description.clone()),
                    category_id: Some(rec.category_id.clone()),
                    note: None,
                    pair_id: None,
                    rec_id: Some(rec.id.clone()),
                    rule_id: None,
                    override_: None,
                    computed: None,
                    vacct_id: None,
                    import_hash: None,
                    created_by: actor_id.to_string(),
                    created_at: String::new(),
                    updated_at: String::new(),
                };
                self.transactions
                    .create(&tx)
                    .await
                    .map_err(|e| RecurrenceError::Internal(e.to_string()))?;
                created += 1;
            }
        }

        Ok(created)
    }

    pub async fn get(&self, actor_id: &str, id: &str) -> Result<Recurrence, RecurrenceError> {
        let rec = self
            .recurrences
            .get_by_id(id)
            .await
            .map_err(|e| RecurrenceError::Internal(e.to_string()))?
            .ok_or(RecurrenceError::NotFound)?;

        self.load_account_checked(actor_id, &rec.account_id, false)
            .await?;

        Ok(rec)
    }

    pub async fn list_for_owner(
        &self,
        actor_id: &str,
        owner_id: &str,
    ) -> Result<Vec<Recurrence>, RecurrenceError> {
        let rows = self
            .recurrences
            .list_for_owner(owner_id)
            .await
            .map_err(|e| RecurrenceError::Internal(e.to_string()))?;

        let mut out = Vec::with_capacity(rows.len());
        for rec in rows {
            if self.may_see_account(actor_id, &rec.account_id).await? {
                out.push(rec);
            }
        }
        Ok(out)
    }

    pub async fn update(
        &self,
        actor_id: &str,
        id: &str,
        cmd: &RecurrenceUpdateCmd,
    ) -> Result<Recurrence, RecurrenceError> {
        self.update_with_report(actor_id, id, cmd)
            .await
            .map(|(rec, _)| rec)
    }

    /// Like [`update`](Self::update), but also re-aligns the already-registered
    /// occurrences when `cmd.apply_to_existing` is set, and reports how many
    /// were rewritten or skipped.
    ///
    /// An occurrence counts as "modified by hand" when it is flagged `override`,
    /// its date is pinned by a per-date override, or its amount differs from the
    /// rule's amount *before* this update. Those are skipped unless
    /// `cmd.overwrite_manual` is set, in which case they (and the pinned
    /// per-date overrides) take the new amount too.
    pub async fn update_with_report(
        &self,
        actor_id: &str,
        id: &str,
        cmd: &RecurrenceUpdateCmd,
    ) -> Result<(Recurrence, AlignReport), RecurrenceError> {
        let mut rec = self
            .recurrences
            .get_by_id(id)
            .await
            .map_err(|e| RecurrenceError::Internal(e.to_string()))?
            .ok_or(RecurrenceError::NotFound)?;

        // The actor must be able to write to the account the rule lives on
        // *now* (otherwise a rule could be pulled out of an account the actor
        // has no right on), and to wherever it is being moved / linked.
        self.load_account_checked(actor_id, &rec.account_id, true)
            .await?;
        let account_id = cmd
            .account_id
            .clone()
            .unwrap_or_else(|| rec.account_id.clone());
        if account_id != rec.account_id {
            self.load_account_checked(actor_id, &account_id, true)
                .await?;
        }
        if let Some(Some(linked)) = &cmd.linked_account_id {
            if rec.linked_account_id.as_ref() != Some(linked) {
                self.load_account_checked(actor_id, linked, true).await?;
            }
        }
        if let Some(Some(cat)) = &cmd.linked_category_id {
            if rec.linked_category_id.as_ref() != Some(cat) {
                self.ensure_category_usable(actor_id, cat).await?;
            }
        }

        let old_amount = rec.amount_cents;

        if let Some(v) = &cmd.description {
            rec.description = v.clone();
        }
        // `category_id` and `kind` are fixed at creation and are not rewritten.
        if let Some(v) = &cmd.amount_cents {
            rec.amount_cents = *v;
        }
        if let Some(v) = &cmd.account_id {
            rec.account_id = v.clone();
        }
        if let Some(v) = &cmd.pattern {
            rec.pattern = v.clone();
        }
        if let Some(v) = &cmd.weekend {
            rec.weekend = *v;
        }
        if let Some(v) = &cmd.first_date {
            rec.first_date = *v;
        }
        if let Some(v) = &cmd.end_type {
            rec.end_type = *v;
        }
        if let Some(v) = &cmd.end_date {
            rec.end_date = *v;
        }
        if let Some(v) = &cmd.end_count {
            rec.end_count = *v;
        }
        if let Some(v) = &cmd.mode {
            rec.mode = *v;
        }
        if let Some(v) = &cmd.avg_n {
            rec.avg_n = *v;
        }
        if let Some(v) = &cmd.active {
            rec.active = *v;
        }
        if let Some(v) = &cmd.note {
            rec.note = v.clone();
        }
        if let Some(v) = &cmd.linked_account_id {
            rec.linked_account_id = v.clone();
        }
        if let Some(v) = &cmd.linked_category_id {
            rec.linked_category_id = v.clone();
        }

        Self::validate_end_fields(rec.end_type, rec.end_date, rec.end_count)?;

        if cmd.apply_to_existing && rec.mode != RecurrenceMode::Fixed {
            return Err(RecurrenceError::Validation(
                "apply_to_existing requires a fixed-amount rule".into(),
            ));
        }

        self.recurrences
            .update(&rec)
            .await
            .map_err(|e| RecurrenceError::Internal(e.to_string()))?;

        // Schedule changes (start date / pattern) never touch the registered
        // rows on their own: the caller picks what to do via `cmd`.
        let created = if cmd.create_missing && rec.active {
            self.materialize_past_occurrences(&rec, actor_id).await?
        } else {
            0
        };
        let (removed, _) = match cmd.stale_action {
            StaleAction::Keep => (0, 0),
            StaleAction::DeleteAuto => self.delete_stale(&rec, old_amount, false).await?,
            StaleAction::DeleteAll => self.delete_stale(&rec, old_amount, true).await?,
        };

        let report = if cmd.apply_to_existing {
            self.align_existing(&rec, old_amount, cmd.apply_from, cmd.overwrite_manual)
                .await?
        } else {
            AlignReport::default()
        };

        Ok((
            rec,
            AlignReport {
                removed,
                created,
                ..report
            },
        ))
    }

    /// Earliest date a materialized occurrence can have: the weekend/holiday
    /// policy may move the first occurrence before `first_date` (e.g. a Saturday
    /// start with the "friday" policy).
    fn schedule_from(rec: &Recurrence) -> NaiveDate {
        rec.first_date - chrono::Duration::days(pfm_domain::SHIFT_MARGIN_DAYS)
    }

    /// Dates (up to today) that the rule, as currently configured, generates.
    async fn current_dates(
        &self,
        rec: &Recurrence,
    ) -> Result<BTreeSet<NaiveDate>, RecurrenceError> {
        let today = chrono::Utc::now().date_naive();
        let from = Self::schedule_from(rec);
        if from > today {
            return Ok(BTreeSet::new());
        }
        let spec = Self::build_spec(rec, vec![])?;
        let calendar = self.holidays.for_user(&rec.owner_id).await;
        Ok(generate_occurrences_in(&spec, from, today, &calendar)
            .into_iter()
            .map(|o| o.date)
            .collect())
    }

    /// Registered rows of `rec` whose date the rule no longer generates, split
    /// into (automatic, modified by hand). `old_amount` is the rule's amount
    /// before the update (a row differing from it counts as modified by hand).
    async fn stale_rows(
        &self,
        rec: &Recurrence,
        old_amount: i64,
    ) -> Result<(Vec<Transaction>, Vec<Transaction>), RecurrenceError> {
        fn internal(e: impl std::fmt::Display) -> RecurrenceError {
            RecurrenceError::Internal(e.to_string())
        }
        let dates = self.current_dates(rec).await?;
        let pinned: BTreeSet<NaiveDate> = self
            .overrides
            .list_for_recurrence(&rec.id)
            .await
            .map_err(internal)?
            .into_iter()
            .map(|o| o.date)
            .collect();
        let txs = self
            .transactions
            .list_for_recurrence(&rec.id, None)
            .await
            .map_err(internal)?;
        let (mut auto, mut manual) = (Vec::new(), Vec::new());
        for tx in txs.into_iter().filter(|t| !dates.contains(&t.date)) {
            let sign = if tx.client_id.ends_with(":pair") {
                -1
            } else {
                1
            };
            if tx.override_ == Some(true)
                || pinned.contains(&tx.date)
                || tx.cents != sign * old_amount
            {
                manual.push(tx);
            } else {
                auto.push(tx);
            }
        }
        Ok((auto, manual))
    }

    /// Delete the rows no longer generated by the rule. Returns
    /// (deleted, kept because modified by hand).
    async fn delete_stale(
        &self,
        rec: &Recurrence,
        old_amount: i64,
        include_manual: bool,
    ) -> Result<(u64, u64), RecurrenceError> {
        let (auto, manual) = self.stale_rows(rec, old_amount).await?;
        let mut victims = auto;
        let kept = if include_manual {
            victims.extend(manual);
            0
        } else {
            manual.len() as u64
        };
        let mut n = 0;
        for tx in victims {
            if self
                .transactions
                .delete(&tx.id)
                .await
                .map_err(|e| RecurrenceError::Internal(e.to_string()))?
            {
                n += 1;
            }
        }
        Ok((n, kept))
    }

    /// Preview of what the schedule in `cmd` (start, pattern, weekend, end) would
    /// do to the registered rows, without writing anything. With an empty `cmd`
    /// it reports the gaps/leftovers of the rule as it is today.
    pub async fn date_impact(
        &self,
        actor_id: &str,
        id: &str,
        cmd: &RecurrenceUpdateCmd,
    ) -> Result<DateImpact, RecurrenceError> {
        let mut rec = self.get(actor_id, id).await?;
        if let Some(v) = cmd.first_date {
            rec.first_date = v;
        }
        if let Some(v) = &cmd.pattern {
            rec.pattern = v.clone();
        }
        if let Some(v) = cmd.weekend {
            rec.weekend = v;
        }
        if let Some(v) = cmd.end_type {
            rec.end_type = v;
        }
        if let Some(v) = cmd.end_date {
            rec.end_date = v;
        }
        if let Some(v) = cmd.end_count {
            rec.end_count = v;
        }
        let rows = self
            .transactions
            .list_for_recurrence(&rec.id, None)
            .await
            .map_err(|e| RecurrenceError::Internal(e.to_string()))?;
        // A date is "present" when everything the rule would write for it exists:
        // the single row, or — for a transfer — both legs. A date with only one
        // surviving leg still counts as missing, so the other leg gets recreated.
        let transfer = rec.linked_account_id.is_some();
        let (mut src, mut lnk): (BTreeSet<NaiveDate>, BTreeSet<NaiveDate>) = Default::default();
        for t in &rows {
            if t.client_id.ends_with(":pair") {
                lnk.insert(t.date);
            } else {
                src.insert(t.date);
            }
        }
        let present = |d: &NaiveDate| {
            if transfer {
                src.contains(d) && lnk.contains(d)
            } else {
                src.contains(d) || lnk.contains(d)
            }
        };
        let to_create = self
            .current_dates(&rec)
            .await?
            .into_iter()
            .filter(|d| !present(d))
            .count() as u64;
        let (auto, manual) = self.stale_rows(&rec, rec.amount_cents).await?;
        // Count occurrences (distinct dates), not transfer legs.
        let occ =
            |v: &[Transaction]| v.iter().map(|t| t.date).collect::<BTreeSet<_>>().len() as u64;
        Ok(DateImpact {
            to_create,
            stale_auto: occ(&auto),
            stale_manual: occ(&manual),
        })
    }

    /// Rewrite the amount of the transactions materialized for `rec` (dated
    /// `>= from`) to the rule's current amount. The linked leg of a transfer
    /// recurrence takes the opposite sign.
    async fn align_existing(
        &self,
        rec: &Recurrence,
        old_amount: i64,
        from: Option<NaiveDate>,
        overwrite_manual: bool,
    ) -> Result<AlignReport, RecurrenceError> {
        fn internal(e: impl std::fmt::Display) -> RecurrenceError {
            RecurrenceError::Internal(e.to_string())
        }

        let txs = self
            .transactions
            .list_for_recurrence(&rec.id, from)
            .await
            .map_err(internal)?;
        let pinned: BTreeSet<NaiveDate> = self
            .overrides
            .list_for_recurrence(&rec.id)
            .await
            .map_err(internal)?
            .into_iter()
            .map(|o| o.date)
            .collect();

        let mut report = AlignReport::default();
        let mut updates: Vec<(String, i64)> = Vec::new();
        for tx in &txs {
            // The linked leg of a transfer recurrence carries the opposite sign.
            let sign = if tx.client_id.ends_with(":pair") {
                -1
            } else {
                1
            };
            let manual = tx.override_ == Some(true)
                || pinned.contains(&tx.date)
                || tx.cents != sign * old_amount;
            if manual && !overwrite_manual {
                report.skipped_manual += 1;
                continue;
            }
            let new_cents = sign * rec.amount_cents;
            if tx.cents != new_cents {
                updates.push((tx.id.clone(), new_cents));
            }
        }

        report.aligned = self
            .transactions
            .set_cents_bulk(&updates)
            .await
            .map_err(internal)?;
        if overwrite_manual {
            self.overrides
                .set_amount_from(&rec.id, from, rec.amount_cents)
                .await
                .map_err(internal)?;
        }
        Ok(report)
    }

    pub async fn delete(&self, actor_id: &str, id: &str) -> Result<(), RecurrenceError> {
        let rec = self
            .recurrences
            .get_by_id(id)
            .await
            .map_err(|e| RecurrenceError::Internal(e.to_string()))?
            .ok_or(RecurrenceError::NotFound)?;

        self.load_account_checked(actor_id, &rec.account_id, true)
            .await?;

        self.recurrences
            .delete(id)
            .await
            .map_err(|e| RecurrenceError::Internal(e.to_string()))?;

        Ok(())
    }

    /// Generate occurrences for a rule over `[from, to]`. Confirmed dates
    /// (overridden transactions + persisted overrides) are not re-generated: the
    /// engine output for those dates is replaced by the confirmed value.
    pub async fn occurrences(
        &self,
        actor_id: &str,
        id: &str,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<RecurrenceOccurrence>, RecurrenceError> {
        let rec = self
            .recurrences
            .get_by_id(id)
            .await
            .map_err(|e| RecurrenceError::Internal(e.to_string()))?
            .ok_or(RecurrenceError::NotFound)?;

        self.load_account_checked(actor_id, &rec.account_id, false)
            .await?;

        let confirmed = self.confirmed_map(id).await?;
        let real: Vec<RealOccurrence> = confirmed
            .iter()
            .map(|(d, a)| RealOccurrence {
                date: *d,
                amount_cents: *a,
            })
            .collect();
        let spec = Self::build_spec(&rec, real)?;

        let calendar = self.holidays.for_user(&rec.owner_id).await;
        let generated = generate_occurrences_in(&spec, from, to, &calendar);

        // Index persisted overrides by date for description lookup.
        let ovs = self
            .overrides
            .list_for_recurrence(id)
            .await
            .map_err(|e| RecurrenceError::Internal(e.to_string()))?;
        let ov_by_date: BTreeMap<NaiveDate, &RecurrenceOverride> =
            ovs.iter().map(|o| (o.date, o)).collect();

        let mut seen: BTreeMap<NaiveDate, RecurrenceOccurrence> = BTreeMap::new();
        for occ in generated {
            if let Some(amt) = confirmed.get(&occ.date) {
                let description = ov_by_date
                    .get(&occ.date)
                    .and_then(|o| o.description.as_deref().filter(|d| !d.is_empty()))
                    .map(|d| d.to_string())
                    .or_else(|| Some(rec.description.clone()));
                let _ = amt;
                let amount = *amt;
                seen.insert(
                    occ.date,
                    RecurrenceOccurrence {
                        date: occ.date,
                        amount_cents: amount,
                        description,
                        overridden: true,
                    },
                );
            } else {
                seen.insert(
                    occ.date,
                    RecurrenceOccurrence {
                        date: occ.date,
                        amount_cents: occ.amount_cents,
                        description: Some(rec.description.clone()),
                        overridden: false,
                    },
                );
            }
        }

        Ok(seen.into_values().collect())
    }

    /// Persist a per-date override for a rule. When `update_rule` is true and the
    /// rule is `fixed`, also re-base the rule's `amount_cents` to the confirmed
    /// value so future occurrences start from it.
    pub async fn set_override(
        &self,
        actor_id: &str,
        rec_id: &str,
        date: NaiveDate,
        cmd: &OverrideCmd,
    ) -> Result<RecurrenceOverride, RecurrenceError> {
        let mut rec = self
            .recurrences
            .get_by_id(rec_id)
            .await
            .map_err(|e| RecurrenceError::Internal(e.to_string()))?
            .ok_or(RecurrenceError::NotFound)?;

        self.load_account_checked(actor_id, &rec.account_id, true)
            .await?;

        let saved = self
            .overrides
            .upsert(rec_id, date, cmd.amount_cents, &cmd.description)
            .await
            .map_err(|e| RecurrenceError::Internal(e.to_string()))?;

        if cmd.update_rule && rec.mode == RecurrenceMode::Fixed {
            rec.amount_cents = cmd.amount_cents;
            self.recurrences
                .update(&rec)
                .await
                .map_err(|e| RecurrenceError::Internal(e.to_string()))?;
        }

        Ok(saved)
    }
}
