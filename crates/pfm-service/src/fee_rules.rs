//! Spending-rule use-cases: CRUD (permission-checked on the rule's account),
//! atomic regeneration of the engine's fee rows, and read-only preview.
//!
//! The pure fee engine ([`pfm_domain::generate_fees`]) only applies a rule to
//! per-period magnitudes; it never touches the ledger. This service supplies
//! those magnitudes by walking the account's *non-engine* base ledger: rows the
//! rules engine produced are excluded via `computed = 1`, so a rule never sees
//! its own (or any other rule's) generated rows. This is what keeps repeated
//! regeneration stable and two active rules order-independent.
//!
//! Regeneration is atomic: the rule's engine rows inside a horizon are deleted
//! and re-created in a single database transaction, and manual rows
//! (`computed` null/`0`, or a different `rule_id`) are never touched.

use std::collections::BTreeMap;

use chrono::{Duration, NaiveDate, Utc};
use pfm_domain::{
    can_see, can_write, fee_horizon_in, generate_fees_in, Account, AccountKind, AccountRule,
    Cadence, CalcType, CondBase, CondOp, FeePeriod, FeeRow, PeriodMetrics, Role, Transaction,
};
use pfm_repo::{
    AccountRulesRepo, AccountsRepo, SharesRepo, SqlxAccountRulesRepo, SqlxAccountsRepo,
    SqlxAuthRepo, SqlxSharesRepo, SqlxTransactionsRepo, Storage, TransactionsRepo, UsersRepo,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FeeRuleError {
    #[error("rule not found")]
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

/// Input for creating a new spending rule on the account named by the route.
pub struct RuleCreateCmd {
    pub name: String,
    pub category_id: String,
    pub calc_type: CalcType,
    pub value: i64,
    pub is_annual: bool,
    pub prorata_days: bool,
    pub annual_cap_cents: Option<i64>,
    pub cadence: Cadence,
    pub charge_day: u32,
    pub charge_month: Option<u32>,
    pub cond_base: CondBase,
    pub cond_op: Option<CondOp>,
    pub cond_threshold: Option<i64>,
    pub active: bool,
}

/// Input for updating an existing rule. All fields `None` = keep the current
/// value. The nullable fields use a double `Option`: `None` = unchanged,
/// `Some(None)` = clear, `Some(Some(..))` = set.
pub struct RuleUpdateCmd {
    pub name: Option<String>,
    pub category_id: Option<String>,
    pub calc_type: Option<CalcType>,
    pub value: Option<i64>,
    pub is_annual: Option<bool>,
    pub prorata_days: Option<bool>,
    pub annual_cap_cents: Option<Option<i64>>,
    pub cadence: Option<Cadence>,
    pub charge_day: Option<u32>,
    pub charge_month: Option<Option<u32>>,
    pub cond_base: Option<CondBase>,
    pub cond_op: Option<Option<CondOp>>,
    pub cond_threshold: Option<Option<i64>>,
    pub active: Option<bool>,
}

/// A (re)generation or preview over a horizon. A missing bound is resolved to
/// the account's full history: `from = start_date`, `to = today`.
pub struct RuleRangeCmd {
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
}

/// The rows a single rule would produce (preview output for one rule).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RulePreview {
    pub rule_id: String,
    pub name: String,
    pub rows: Vec<FeeRow>,
}

pub struct FeeRuleService {
    rules: SqlxAccountRulesRepo,
    transactions: SqlxTransactionsRepo,
    accounts: SqlxAccountsRepo,
    users: SqlxAuthRepo,
    shares: SqlxSharesRepo,
    holidays: crate::holidays::HolidayService,
    category_access: crate::CategoryAccess,
}

impl FeeRuleService {
    pub fn new(storage: &Storage) -> Self {
        Self {
            rules: SqlxAccountRulesRepo::new(storage),
            transactions: SqlxTransactionsRepo::new(storage),
            accounts: SqlxAccountsRepo::new(storage),
            users: SqlxAuthRepo::new(storage),
            shares: SqlxSharesRepo::new(storage),
            holidays: crate::holidays::HolidayService::new(storage),
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
    ) -> Result<(), FeeRuleError> {
        self.category_access
            .ensure_usable(actor_id, category_id)
            .await
            .map_err(|e| match e {
                crate::CategoryAccessError::NotFound => {
                    FeeRuleError::Validation("unknown category".into())
                }
                crate::CategoryAccessError::Internal(m) => FeeRuleError::Internal(m),
            })
    }

    async fn check_access(
        &self,
        actor_id: &str,
        account: &Account,
        write: bool,
    ) -> Result<bool, FeeRuleError> {
        if account.owner_id == actor_id {
            return Ok(true);
        }

        let actor = self
            .users
            .get_by_id(actor_id)
            .await
            .map_err(|e| FeeRuleError::Internal(e.to_string()))?
            .ok_or_else(|| FeeRuleError::Internal("actor not found".into()))?;

        if actor.role == Role::Admin {
            return Ok(true);
        }

        let share = self
            .shares
            .get(&account.id, actor_id)
            .await
            .map_err(|e| FeeRuleError::Internal(e.to_string()))?;

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

    /// Load the account at `account_id` and enforce access for `actor`.
    async fn load_account_checked(
        &self,
        actor_id: &str,
        account_id: &str,
        write: bool,
    ) -> Result<Account, FeeRuleError> {
        let account = self
            .accounts
            .get_by_id(account_id)
            .await
            .map_err(|e| FeeRuleError::Internal(e.to_string()))?
            .ok_or(FeeRuleError::AccountNotFound)?;

        if !self.check_access(actor_id, &account, write).await? {
            return Err(FeeRuleError::Forbidden);
        }

        Ok(account)
    }

    /// Load the rule at `rule_id` and confirm it belongs to `account` (a rule on
    /// a different account is a 404, not a cross-account operation).
    async fn load_rule_on(
        &self,
        rule_id: &str,
        account: &Account,
    ) -> Result<AccountRule, FeeRuleError> {
        let rule = self
            .rules
            .get_by_id(rule_id)
            .await
            .map_err(|e| FeeRuleError::Internal(e.to_string()))?
            .ok_or(FeeRuleError::NotFound)?;

        if rule.account_id != account.id {
            return Err(FeeRuleError::NotFound);
        }
        Ok(rule)
    }

    fn validate(rule: &AccountRule) -> Result<(), FeeRuleError> {
        if rule.name.trim().is_empty() {
            return Err(FeeRuleError::Validation("name must not be empty".into()));
        }
        if rule.value < 0 {
            return Err(FeeRuleError::Validation("value must be >= 0".into()));
        }
        if rule.charge_day > 31 {
            return Err(FeeRuleError::Validation(
                "charge_day must be between 0 and 31".into(),
            ));
        }
        if let Some(m) = rule.charge_month {
            if !(1..=12).contains(&m) {
                return Err(FeeRuleError::Validation(
                    "charge_month must be between 1 and 12".into(),
                ));
            }
        }
        if let Some(cap) = rule.annual_cap_cents {
            if cap < 0 {
                return Err(FeeRuleError::Validation(
                    "annual_cap_cents must be >= 0".into(),
                ));
            }
        }
        Ok(())
    }

    /// Create a new rule on `account_id`.
    pub async fn create(
        &self,
        actor_id: &str,
        account_id: &str,
        cmd: &RuleCreateCmd,
    ) -> Result<AccountRule, FeeRuleError> {
        let account = self
            .load_account_checked(actor_id, account_id, true)
            .await?;
        self.ensure_category_usable(actor_id, &cmd.category_id)
            .await?;

        let rule = AccountRule {
            id: String::new(),
            account_id: account.id.clone(),
            name: cmd.name.clone(),
            category_id: cmd.category_id.clone(),
            calc_type: cmd.calc_type,
            value: cmd.value,
            is_annual: cmd.is_annual,
            prorata_days: cmd.prorata_days,
            annual_cap_cents: cmd.annual_cap_cents,
            cadence: cmd.cadence,
            charge_day: cmd.charge_day,
            charge_month: cmd.charge_month,
            cond_base: cmd.cond_base,
            cond_op: cmd.cond_op,
            cond_threshold: cmd.cond_threshold,
            active: cmd.active,
            created_at: String::new(),
            updated_at: String::new(),
        };
        Self::validate(&rule)?;

        let created = self
            .rules
            .create(&rule)
            .await
            .map_err(|e| FeeRuleError::Internal(e.to_string()))?;
        self.align_rule(&created, &account, actor_id).await;
        Ok(created)
    }

    /// Bring the rule's engine rows in line with its current definition:
    /// regenerate when active, drop them when paused. Cards are skipped (their
    /// oneri are applied by the settlement engine). Failures are logged only:
    /// the rule itself is already saved and the periodic pass will retry.
    async fn align_rule(&self, rule: &AccountRule, account: &Account, actor_id: &str) {
        if account.kind == AccountKind::Card {
            return;
        }
        if !rule.active {
            if let Err(e) = self
                .transactions
                .delete_rule_rows(&rule.id, &account.id)
                .await
            {
                tracing::warn!(rule_id = %rule.id, "rule align: delete rows failed: {e}");
            }
            return;
        }
        let cmd = RuleRangeCmd {
            from: None,
            to: None,
        };
        match self.compute_rows(rule, account, &cmd).await {
            Ok(Some((from, to, rows))) => {
                if let Err(e) = self
                    .transactions
                    .regenerate_rule_rows(rule, actor_id, from, to, &rows)
                    .await
                {
                    tracing::warn!(rule_id = %rule.id, "rule align: regenerate failed: {e}");
                }
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(rule_id = %rule.id, "rule align: compute failed: {e}"),
        }
    }

    /// Fetch a rule belonging to `account_id`.
    pub async fn get(
        &self,
        actor_id: &str,
        account_id: &str,
        rule_id: &str,
    ) -> Result<AccountRule, FeeRuleError> {
        let account = self
            .load_account_checked(actor_id, account_id, false)
            .await?;
        self.load_rule_on(rule_id, &account).await
    }

    /// All rules on `account_id`.
    pub async fn list_for_account(
        &self,
        actor_id: &str,
        account_id: &str,
    ) -> Result<Vec<AccountRule>, FeeRuleError> {
        self.load_account_checked(actor_id, account_id, false)
            .await?;
        self.rules
            .list_for_account(account_id)
            .await
            .map_err(|e| FeeRuleError::Internal(e.to_string()))
    }

    /// Update mutable fields on a rule belonging to `account_id`.
    pub async fn update(
        &self,
        actor_id: &str,
        account_id: &str,
        rule_id: &str,
        cmd: &RuleUpdateCmd,
    ) -> Result<AccountRule, FeeRuleError> {
        let account = self
            .load_account_checked(actor_id, account_id, true)
            .await?;
        let mut rule = self.load_rule_on(rule_id, &account).await?;

        if let Some(v) = &cmd.name {
            rule.name = v.clone();
        }
        if let Some(v) = &cmd.category_id {
            if *v != rule.category_id {
                self.ensure_category_usable(actor_id, v).await?;
            }
            rule.category_id = v.clone();
        }
        if let Some(v) = cmd.calc_type {
            rule.calc_type = v;
        }
        if let Some(v) = cmd.value {
            rule.value = v;
        }
        if let Some(v) = cmd.is_annual {
            rule.is_annual = v;
        }
        if let Some(v) = cmd.prorata_days {
            rule.prorata_days = v;
        }
        if let Some(v) = &cmd.annual_cap_cents {
            rule.annual_cap_cents = *v;
        }
        if let Some(v) = cmd.cadence {
            rule.cadence = v;
        }
        if let Some(v) = cmd.charge_day {
            rule.charge_day = v;
        }
        if let Some(v) = &cmd.charge_month {
            rule.charge_month = *v;
        }
        if let Some(v) = cmd.cond_base {
            rule.cond_base = v;
        }
        if let Some(v) = &cmd.cond_op {
            rule.cond_op = *v;
        }
        if let Some(v) = &cmd.cond_threshold {
            rule.cond_threshold = *v;
        }
        if let Some(v) = cmd.active {
            rule.active = v;
        }

        Self::validate(&rule)?;
        self.rules
            .update(&rule)
            .await
            .map_err(|e| FeeRuleError::Internal(e.to_string()))?;
        self.align_rule(&rule, &account, actor_id).await;
        Ok(rule)
    }

    /// Delete a rule belonging to `account_id` and remove its engine rows so no
    /// orphaned fee transactions remain. Manual rows are untouched.
    pub async fn delete(
        &self,
        actor_id: &str,
        account_id: &str,
        rule_id: &str,
    ) -> Result<(), FeeRuleError> {
        let account = self
            .load_account_checked(actor_id, account_id, true)
            .await?;
        let rule = self.load_rule_on(rule_id, &account).await?;

        self.transactions
            .delete_rule_rows(&rule.id, &account.id)
            .await
            .map_err(|e| FeeRuleError::Internal(e.to_string()))?;
        self.rules
            .delete(rule_id)
            .await
            .map_err(|e| FeeRuleError::Internal(e.to_string()))?;
        Ok(())
    }

    /// (Re)generate the rule's fee rows over the given horizon and persist them
    /// atomically. Returns the rows that were materialized.
    pub async fn generate(
        &self,
        actor_id: &str,
        account_id: &str,
        rule_id: &str,
        cmd: &RuleRangeCmd,
    ) -> Result<Vec<FeeRow>, FeeRuleError> {
        let account = self
            .load_account_checked(actor_id, account_id, true)
            .await?;
        let rule = self.load_rule_on(rule_id, &account).await?;

        // A card's rules never materialize rows on the card ledger: their oneri
        // are applied to the backing account by the settlement engine, per cycle.
        if account.kind == AccountKind::Card {
            return Ok(Vec::new());
        }

        let Some((from, to, rows)) = self.compute_rows(&rule, &account, cmd).await? else {
            return Ok(Vec::new());
        };

        self.transactions
            .regenerate_rule_rows(&rule, actor_id, from, to, &rows)
            .await
            .map_err(|e| FeeRuleError::Internal(e.to_string()))?;

        Ok(rows)
    }

    /// (Re)generate the fee rows for every active rule on `account_id` and
    /// persist them atomically, rule by rule. Used before a manual card
    /// settlement so that rule-defined movements (annual fees, stamp duty,
    /// etc.) on the account are materialized before the cycle total is
    /// computed — mirroring what the periodic scheduler already does before
    /// its own catch-up pass. Returns the combined rows materialized across
    /// all rules.
    pub async fn generate_all_for_account(
        &self,
        actor_id: &str,
        account_id: &str,
    ) -> Result<Vec<FeeRow>, FeeRuleError> {
        let account = self
            .load_account_checked(actor_id, account_id, true)
            .await?;

        // A card's rules never materialize rows on the card ledger: their oneri
        // are applied to the backing account by the settlement engine, per cycle.
        if account.kind == AccountKind::Card {
            return Ok(Vec::new());
        }

        let rules = self
            .rules
            .list_for_account(account_id)
            .await
            .map_err(|e| FeeRuleError::Internal(e.to_string()))?;

        let cmd = RuleRangeCmd {
            from: None,
            to: None,
        };

        let mut all_rows = Vec::new();
        for rule in rules.iter().filter(|r| r.active) {
            if let Some((from, to, rows)) = self.compute_rows(rule, &account, &cmd).await? {
                self.transactions
                    .regenerate_rule_rows(rule, actor_id, from, to, &rows)
                    .await
                    .map_err(|e| FeeRuleError::Internal(e.to_string()))?;
                all_rows.extend(rows);
            }
        }
        Ok(all_rows)
    }

    /// Materialize due fee rows for every active rule across all accounts,
    /// without any permission check. Intended for the background scheduler.
    ///
    /// Each rule is regenerated over its full horizon (`start_date → today`).
    /// `regenerate_rule_rows` is atomic per rule, so partial failures leave the
    /// ledger in a consistent state for the rules that succeeded.
    pub async fn materialize_all_pending(&self) {
        let accounts = match self.accounts.list_all().await {
            Ok(a) => a,
            Err(e) => {
                tracing::error!("scheduler: failed to list accounts: {e}");
                return;
            }
        };

        let cmd = RuleRangeCmd {
            from: None,
            to: None,
        };

        for account in &accounts {
            // A card's rules are consumed by the card settlement engine (applied
            // to the backing account per cycle), never materialized on the card.
            if account.kind == AccountKind::Card {
                continue;
            }
            let rules = match self.rules.list_for_account(&account.id).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!(
                        account_id = %account.id,
                        "scheduler: failed to list rules: {e}"
                    );
                    continue;
                }
            };

            for rule in rules.iter().filter(|r| r.active) {
                let result = self.compute_rows(rule, account, &cmd).await;
                match result {
                    Ok(Some((from, to, rows))) => {
                        if let Err(e) = self
                            .transactions
                            .regenerate_rule_rows(rule, "system", from, to, &rows)
                            .await
                        {
                            tracing::error!(
                                rule_id = %rule.id,
                                "scheduler: regenerate_rule_rows failed: {e}"
                            );
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!(
                            rule_id = %rule.id,
                            "scheduler: compute_rows failed: {e}"
                        );
                    }
                }
            }
        }
    }

    /// Preview the fees that a horizon would produce, without writing. When
    /// `rule_id` is given, previews just that rule; otherwise previews every
    /// active rule on the account.
    pub async fn preview_rules(
        &self,
        actor_id: &str,
        account_id: &str,
        rule_id: Option<&str>,
        cmd: &RuleRangeCmd,
    ) -> Result<Vec<RulePreview>, FeeRuleError> {
        let account = self
            .load_account_checked(actor_id, account_id, false)
            .await?;
        let all = self
            .rules
            .list_for_account(account_id)
            .await
            .map_err(|e| FeeRuleError::Internal(e.to_string()))?;

        let targets: Vec<AccountRule> = match rule_id {
            Some(rid) => vec![self.load_rule_on(rid, &account).await?],
            None => all.into_iter().filter(|r| r.active).collect(),
        };

        let mut out = Vec::with_capacity(targets.len());
        for rule in &targets {
            let rows = self
                .compute_rows(rule, &account, cmd)
                .await?
                .map(|(_, _, rows)| rows)
                .unwrap_or_default();
            out.push(RulePreview {
                rule_id: rule.id.clone(),
                name: rule.name.clone(),
                rows,
            });
        }
        Ok(out)
    }

    /// Compute the fee rows a rule would produce over `cmd`'s horizon. Returns
    /// `(from, to, rows)` or `None` when the rule has no charge deadline inside
    /// the horizon (in which case there is nothing to compute or materialize).
    async fn compute_rows(
        &self,
        rule: &AccountRule,
        account: &Account,
        cmd: &RuleRangeCmd,
    ) -> Result<Option<(NaiveDate, NaiveDate, Vec<FeeRow>)>, FeeRuleError> {
        let today = Utc::now().date_naive();
        // Default horizon starts at the earliest ledger row when that predates
        // the account's stored `start_date` (imported history), so quarterly or
        // annual charges are not skipped for periods the ledger already covers.
        let from = match cmd.from {
            Some(f) => f,
            None => match self
                .transactions
                .min_noncomputed_date(&account.id)
                .await
                .map_err(|e| FeeRuleError::Internal(e.to_string()))?
            {
                Some(min) => account.start_date.min(min),
                None => account.start_date,
            },
        };
        let to = cmd.to.unwrap_or(today);
        if from > to {
            return Ok(None);
        }

        let fee_rule = rule.fee_rule();
        let calendar = self.holidays.for_user(&account.owner_id).await;
        let Some((data_from, data_to)) = fee_horizon_in(&fee_rule, from, to, &calendar) else {
            return Ok(None);
        };

        // Base balance at the start of the data range: the account opening
        // balance plus everything the manual ledger had accrued before it.
        let base = account.start_balance_cents
            + self
                .transactions
                .sum_noncomputed_cents_before(&account.id, data_from)
                .await
                .map_err(|e| FeeRuleError::Internal(e.to_string()))?;

        let rows = self
            .transactions
            .list_noncomputed(&account.id, data_from, data_to)
            .await
            .map_err(|e| FeeRuleError::Internal(e.to_string()))?;

        let generated = generate_fees_in(&fee_rule, from, to, &calendar, |p| {
            period_metrics(&rows, base, p)
        });

        Ok(Some((from, to, generated)))
    }
}

/// The already-computed magnitudes for one competence period, derived purely
/// from the account's non-engine base ledger.
///
/// `rows` is the sorted list of non-computed transactions within the preloaded
/// data range. `opening` is the balance carried into the start of that range
/// (the account's opening balance plus the manual ledger accrued before it).
/// Engine rows are absent by construction, so a period's metrics never reflect
/// any rule's output.
fn period_metrics(rows: &[Transaction], opening: i64, p: &FeePeriod) -> PeriodMetrics {
    // Per-day net (for the running-balance walk) plus the gross flows and the
    // transaction count, over the period. Rows dated before the period roll
    // into the balance the period starts from.
    let mut daily_net: BTreeMap<NaiveDate, i64> = BTreeMap::new();
    let mut pre_period: i64 = 0;
    let mut total_out: i64 = 0;
    let mut total_in: i64 = 0;
    let mut tx_count: i64 = 0;

    for r in rows {
        if r.date < p.from {
            pre_period += r.cents;
        } else if r.date <= p.to {
            *daily_net.entry(r.date).or_insert(0) += r.cents;
            tx_count += 1;
            if r.cents > 0 {
                total_in += r.cents;
            } else {
                total_out += -r.cents;
            }
        }
    }

    let mut running = (opening + pre_period) as i128;
    let mut sum_daily: i128 = 0;
    let mut day = p.from;
    while day <= p.to {
        if let Some(net) = daily_net.get(&day) {
            running += *net as i128;
        }
        sum_daily += running;
        day += Duration::days(1);
    }

    let day_count = (p.to - p.from).num_days() + 1;
    PeriodMetrics {
        avg_balance_cents: round_div(sum_daily, day_count as i128),
        end_balance_cents: running.clamp(i64::MIN as i128, i64::MAX as i128) as i64,
        total_out_cents: total_out,
        total_in_cents: total_in,
        tx_count,
    }
}

/// `num / den` rounded half away from zero, computed in `i128` to avoid
/// overflow on large balance sums, then clamped to `i64`. `den` must be `> 0`.
fn round_div(num: i128, den: i128) -> i64 {
    debug_assert!(den > 0);
    let sign = if num < 0 { -1i128 } else { 1i128 };
    let rounded = (2 * num.abs() + den) / (2 * den);
    (sign * rounded).clamp(i64::MIN as i128, i64::MAX as i128) as i64
}
