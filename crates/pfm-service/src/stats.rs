//! Statistics use-cases: KPIs, expenses by category, balance trend with
//! future projection (recurrence + fee engines), and budget progress.
//!
//! The balance trend's historical part is derived from real transaction rows
//! (the same data the register uses). The future part is a runtime projection:
//! for each active recurrence on the account the pure [`generate_occurrences`]
//! engine is called, and for each active spending rule the pure
//! [`generate_fees`] engine is called. Neither writes to the database; the
//! projected points are returned with `is_forecast = true`.
//!
//! The budget comparison uses the same-length previous period (a 31-day
//! period in March is compared against the 31 days preceding it, not against
//! "last month"), so months of 28/29/30/31 days are all handled correctly.

use std::collections::BTreeMap;

use chrono::{Datelike, Duration, NaiveDate, Utc};
use pfm_domain::{
    budget_progress, build_register, can_see, can_write, fee_horizon_in, generate_fees_in,
    generate_occurrences_in, Account, CategoryKind, FeePeriod, PeriodMetrics, RealOccurrence,
    RecurrenceMode, RecurrenceSpec, Role,
};
use pfm_repo::{
    AccountRulesRepo, AccountsRepo, CategoriesRepo, OverridesRepo, PrefsRepo, RecurrencesRepo,
    RegisterFilters, SharesRepo, SqlxAccountRulesRepo, SqlxAccountsRepo, SqlxAuthRepo,
    SqlxCategoriesRepo, SqlxOverridesRepo, SqlxRecurrencesRepo, SqlxSharesRepo,
    SqlxTransactionsRepo, Storage, TransactionsRepo, UsersRepo,
};
use thiserror::Error;

use crate::recurrences::RecurrenceService;

#[derive(Debug, Error)]
pub enum StatsError {
    #[error("account not found")]
    AccountNotFound,
    #[error("forbidden")]
    Forbidden,
    #[error("validation error: {0}")]
    Validation(String),
    #[error("internal error: {0}")]
    Internal(String),
}

/// Key performance indicators for the requested period.
#[derive(Debug, Clone)]
pub struct KpiSummary {
    /// Total inflow (positive cents) over the period.
    pub income_cents: i64,
    /// Total outflow (positive magnitude) over the period.
    pub expense_cents: i64,
    /// `closing_cents - opening_cents` (net change over the period, including
    /// the future projection when the period extends past today).
    pub net_cents: i64,
    /// Account balance at the start of the period.
    pub opening_cents: i64,
    /// Account balance at the end of the period (projected when in the future).
    pub closing_cents: i64,
}

/// Total outflow in one category over the period.
#[derive(Debug, Clone)]
pub struct CategorySpend {
    pub category_id: String,
    pub category_name: String,
    /// Total outflow (positive magnitude) in the category over the period.
    pub cents: i64,
}

/// A single point on the balance trend.
#[derive(Debug, Clone)]
pub struct BalancePoint {
    pub date: NaiveDate,
    pub balance_cents: i64,
    /// `true` when the point is a runtime projection (future date), not a real
    /// historical balance.
    pub is_forecast: bool,
}

/// The full statistics payload for one account over a date range.
#[derive(Debug, Clone)]
pub struct StatsResult {
    /// Effective start of the computed range (clamped to the account's
    /// `start_date`).
    pub from: NaiveDate,
    /// Effective end of the computed range.
    pub to: NaiveDate,
    pub kpi: KpiSummary,
    pub expenses_by_category: Vec<CategorySpend>,
    pub balance_trend: Vec<BalancePoint>,
    pub budget: Vec<pfm_domain::BudgetProgress>,
}

pub struct StatsService {
    transactions: SqlxTransactionsRepo,
    accounts: SqlxAccountsRepo,
    users: SqlxAuthRepo,
    shares: SqlxSharesRepo,
    categories: SqlxCategoriesRepo,
    recurrences: SqlxRecurrencesRepo,
    rules: SqlxAccountRulesRepo,
    overrides: SqlxOverridesRepo,
    holidays: crate::holidays::HolidayService,
    cards: crate::card::CardService,
}

impl StatsService {
    pub fn new(storage: &Storage) -> Self {
        Self {
            transactions: SqlxTransactionsRepo::new(storage),
            accounts: SqlxAccountsRepo::new(storage),
            users: SqlxAuthRepo::new(storage),
            shares: SqlxSharesRepo::new(storage),
            categories: SqlxCategoriesRepo::new(storage),
            recurrences: SqlxRecurrencesRepo::new(storage),
            rules: SqlxAccountRulesRepo::new(storage),
            overrides: SqlxOverridesRepo::new(storage),
            holidays: crate::holidays::HolidayService::new(storage),
            cards: crate::card::CardService::new(storage),
        }
    }

    async fn check_access(
        &self,
        actor_id: &str,
        account: &Account,
        write: bool,
    ) -> Result<bool, StatsError> {
        if account.owner_id == actor_id {
            return Ok(true);
        }

        let actor = self
            .users
            .get_by_id(actor_id)
            .await
            .map_err(|e| StatsError::Internal(e.to_string()))?
            .ok_or_else(|| StatsError::Internal("actor not found".into()))?;

        if actor.role == Role::Admin {
            return Ok(true);
        }

        let share = self
            .shares
            .get(&account.id, actor_id)
            .await
            .map_err(|e| StatsError::Internal(e.to_string()))?;

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

    /// Resolve the effective date range. Explicit `from`/`to` take precedence
    /// over `period`; when neither is given the current month is used.
    fn resolve_range(
        from: Option<NaiveDate>,
        to: Option<NaiveDate>,
        period: Option<&str>,
        today: NaiveDate,
    ) -> (NaiveDate, NaiveDate) {
        if let (Some(f), Some(t)) = (from, to) {
            return (f, t);
        }
        let p = period.unwrap_or("month");
        if p == "year" {
            let f = NaiveDate::from_ymd_opt(today.year(), 1, 1).expect("Jan 1 always valid");
            let t = NaiveDate::from_ymd_opt(today.year(), 12, 31).expect("Dec 31 always valid");
            (f, t)
        } else {
            let month = today.month();
            let f = NaiveDate::from_ymd_opt(today.year(), month, 1).expect("day 1 valid");
            let next_month = if month == 12 {
                (today.year() + 1, 1)
            } else {
                (today.year(), month + 1)
            };
            let last = NaiveDate::from_ymd_opt(next_month.0, next_month.1, 1).expect("day 1 valid")
                - Duration::days(1);
            (f, last)
        }
    }

    /// Compute the full statistics payload for `account_id` over the date range
    /// derived from `from`/`to` (or `period`). `filters` narrow the
    /// expenses-by-category breakdown; the KPIs, balance trend and budget are
    /// always computed from the full (unfiltered) account ledger.
    #[allow(clippy::too_many_arguments)]
    pub async fn stats(
        &self,
        actor_id: &str,
        account_id: &str,
        from: Option<NaiveDate>,
        to: Option<NaiveDate>,
        period: Option<&str>,
        filters: &RegisterFilters,
    ) -> Result<StatsResult, StatsError> {
        let account = self
            .accounts
            .get_by_id(account_id)
            .await
            .map_err(|e| StatsError::Internal(e.to_string()))?
            .ok_or(StatsError::AccountNotFound)?;

        if !self.check_access(actor_id, &account, false).await? {
            return Err(StatsError::Forbidden);
        }

        let today = Utc::now().date_naive();
        let (raw_from, raw_to) = Self::resolve_range(from, to, period, today);
        if raw_from > raw_to {
            return Err(StatsError::Validation("from must be <= to".into()));
        }

        let effective_from = raw_from.max(account.start_date);
        let effective_to = raw_to.max(effective_from);

        let rows = self
            .transactions
            .list_for_register(
                account_id,
                effective_from,
                effective_to,
                &RegisterFilters::default(),
            )
            .await
            .map_err(|e| StatsError::Internal(e.to_string()))?;

        let pre_sum = self
            .transactions
            .sum_cents_before_filtered(account_id, effective_from, None)
            .await
            .map_err(|e| StatsError::Internal(e.to_string()))?;
        let opening_balance = account.start_balance_cents + pre_sum;

        let prefs = self
            .users
            .get(actor_id)
            .await
            .map_err(|e| StatsError::Internal(e.to_string()))?;

        let mut running = opening_balance;
        let mut rows_with_balance: Vec<pfm_domain::TransactionRow> = Vec::with_capacity(rows.len());
        for tx in &rows {
            running += tx.cents;
            rows_with_balance.push(pfm_domain::TransactionRow {
                tx: tx.clone(),
                running_balance_cents: running,
            });
        }

        let register = build_register(
            opening_balance,
            rows_with_balance,
            effective_from,
            effective_to,
            today,
            prefs.show_future,
        );

        let mut trend: Vec<BalancePoint> = register
            .days
            .iter()
            .filter(|d| d.date <= today)
            .map(|d| BalancePoint {
                date: d.date,
                balance_cents: d.closing_balance_cents,
                is_forecast: false,
            })
            .collect();

        if effective_to > today {
            let base = trend
                .last()
                .map(|p| p.balance_cents)
                .unwrap_or(opening_balance);
            let projected = self
                .future_projection(account_id, base, today + Duration::days(1), effective_to)
                .await;
            trend.extend(projected);
        }

        let closing = trend
            .last()
            .map(|p| p.balance_cents)
            .unwrap_or(opening_balance);

        let kpi_rows = self
            .transactions
            .list_for_register(
                account_id,
                raw_from.max(account.start_date),
                raw_to,
                &RegisterFilters::default(),
            )
            .await
            .map_err(|e| StatsError::Internal(e.to_string()))?;
        let filtered_rows = self
            .transactions
            .list_for_register(
                account_id,
                raw_from.max(account.start_date),
                raw_to,
                filters,
            )
            .await
            .map_err(|e| StatsError::Internal(e.to_string()))?;

        let (mut income, mut expense) = (0i64, 0i64);
        for tx in &kpi_rows {
            if tx.cents > 0 {
                income += tx.cents;
            } else {
                expense += -tx.cents;
            }
        }

        let all_cats = self
            .categories
            .list()
            .await
            .map_err(|e| StatsError::Internal(e.to_string()))?;
        let cat_name: BTreeMap<&str, &str> = all_cats
            .iter()
            .map(|c| (c.id.as_str(), c.name.as_str()))
            .collect();

        let mut cat_map: BTreeMap<String, i64> = BTreeMap::new();
        for tx in &filtered_rows {
            if tx.cents < 0 {
                if let Some(cat_id) = &tx.category_id {
                    *cat_map.entry(cat_id.clone()).or_insert(0) += -tx.cents;
                }
            }
        }

        let expenses_by_category: Vec<CategorySpend> = cat_map
            .iter()
            .filter(|(_, &spend)| spend > 0)
            .map(|(cat_id, spend)| CategorySpend {
                category_id: cat_id.clone(),
                category_name: cat_name.get(cat_id.as_str()).unwrap_or(&"").to_string(),
                cents: *spend,
            })
            .collect();

        let (prev_from, prev_to) = pfm_domain::prev_period_range(raw_from, raw_to);
        let prev_rows = self
            .transactions
            .list_for_register(
                account_id,
                prev_from.max(account.start_date),
                prev_to,
                &RegisterFilters::default(),
            )
            .await
            .map_err(|e| StatsError::Internal(e.to_string()))?;

        let budget: Vec<pfm_domain::BudgetProgress> = all_cats
            .iter()
            .filter(|c| c.budget_cents.is_some())
            .filter(|c| c.kind == CategoryKind::Out)
            .map(|c| {
                let budget = c.budget_cents.unwrap_or(0);
                let spent = spent_in_category(&kpi_rows, &c.id, raw_from, raw_to);
                let prev = spent_in_category(&prev_rows, &c.id, prev_from, prev_to);
                budget_progress(&c.id, &c.name, budget, spent, prev)
            })
            .collect();

        Ok(StatsResult {
            from: effective_from,
            to: effective_to,
            kpi: KpiSummary {
                income_cents: income,
                expense_cents: expense,
                net_cents: closing - opening_balance,
                opening_cents: opening_balance,
                closing_cents: closing,
            },
            expenses_by_category,
            balance_trend: trend,
            budget,
        })
    }

    /// Project the account balance for each day in `[from, to]` using the
    /// account's active recurrences and spending rules. The projection starts
    /// from `base_cents` (the last known historical balance) and chains day by
    /// day. No rows are written to the database.
    ///
    /// The projection is two-pass so fee rules see the true balance: the first
    /// pass folds the recurrence amounts into running balances, and those
    /// recurrence-inclusive balances are what the fee engine's per-period
    /// metrics are derived from (a fee never sees its own output). The fee rows
    /// are then added and the balance is re-chained to produce the final series.
    async fn future_projection(
        &self,
        account_id: &str,
        base_cents: i64,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Vec<BalancePoint> {
        if from > to {
            return Vec::new();
        }

        let mut per_day: BTreeMap<NaiveDate, i64> = BTreeMap::new();

        let _ = self
            .projected_from_recurrences(account_id, from, to, &mut per_day)
            .await;
        let _ = self
            .projected_card_charges(account_id, from, to, &mut per_day)
            .await;

        // Running balances including recurrences (but not fees): the base the
        // fee engine's metrics are derived from.
        let with_recs = project_daily(per_day.clone(), base_cents, from, to);
        let balances: BTreeMap<NaiveDate, i64> = with_recs
            .iter()
            .map(|p| (p.date, p.balance_cents))
            .collect();

        let _ = self
            .projected_fees(account_id, from, to, &balances, &mut per_day)
            .await;

        // Final series, now including the projected fee charges.
        project_daily(per_day, base_cents, from, to)
    }

    /// Add the expected debits of the credit cards backed by `account_id` (cycles
    /// not yet settled) to `per_day`, on their business-day-adjusted charge date.
    async fn projected_card_charges(
        &self,
        account_id: &str,
        from: NaiveDate,
        to: NaiveDate,
        per_day: &mut BTreeMap<NaiveDate, i64>,
    ) -> Result<(), StatsError> {
        let today = chrono::Utc::now().date_naive();
        let cards = self
            .accounts
            .list_active_cards()
            .await
            .map_err(|e| StatsError::Internal(e.to_string()))?;
        for card in cards
            .iter()
            .filter(|c| c.primary_account_id.as_deref() == Some(account_id))
        {
            let forecast = self
                .cards
                .forecast_for_card(card, from, to, today)
                .await
                .map_err(|e| StatsError::Internal(e.to_string()))?;
            for f in forecast {
                if f.backing_date >= from && f.backing_date <= to && f.backing_date > today {
                    *per_day.entry(f.backing_date).or_insert(0) -= f.spend_cents + f.fee_cents;
                }
            }
        }
        Ok(())
    }

    /// Add the projected amounts of every active recurrence on the account to
    /// `per_day`. Recurrences whose pattern cannot be parsed are skipped
    /// (they cannot be projected reliably).
    async fn projected_from_recurrences(
        &self,
        account_id: &str,
        from: NaiveDate,
        to: NaiveDate,
        per_day: &mut BTreeMap<NaiveDate, i64>,
    ) -> Result<(), StatsError> {
        let recs = self
            .recurrences
            .list_for_account(account_id)
            .await
            .map_err(|e| StatsError::Internal(e.to_string()))?;

        for rec in &recs {
            if !rec.active {
                continue;
            }
            if rec.first_date > to {
                continue;
            }

            let real: Vec<RealOccurrence> = if rec.mode == RecurrenceMode::Avg {
                let tx_reals = self
                    .transactions
                    .real_occurrences_for_recurrence(&rec.id)
                    .await
                    .map_err(|e| StatsError::Internal(e.to_string()))?;
                let ovs = self
                    .overrides
                    .list_for_recurrence(&rec.id)
                    .await
                    .map_err(|e| StatsError::Internal(e.to_string()))?;
                let mut map: BTreeMap<NaiveDate, i64> = BTreeMap::new();
                for r in tx_reals {
                    map.insert(r.date, r.amount_cents);
                }
                for o in ovs {
                    map.insert(o.date, o.amount_cents);
                }
                map.into_iter()
                    .map(|(d, a)| RealOccurrence {
                        date: d,
                        amount_cents: a,
                    })
                    .collect()
            } else {
                Vec::new()
            };

            let spec: RecurrenceSpec = match RecurrenceService::build_spec(rec, real) {
                Ok(s) => s,
                Err(_) => continue,
            };

            let calendar = self.holidays.for_user(&rec.owner_id).await;
            for occ in generate_occurrences_in(&spec, from, to, &calendar) {
                *per_day.entry(occ.date).or_insert(0) += occ.amount_cents;
            }
        }

        Ok(())
    }

    /// Add the projected fee charges of every active spending rule on the
    /// account to `per_day`. The fee engine's per-period metrics are derived
    /// from `balances` — the running-balance series that already includes the
    /// projected recurrences — so balance-dependent fees use the projected
    /// balance rather than real historical data.
    async fn projected_fees(
        &self,
        account_id: &str,
        from: NaiveDate,
        to: NaiveDate,
        balances: &BTreeMap<NaiveDate, i64>,
        per_day: &mut BTreeMap<NaiveDate, i64>,
    ) -> Result<(), StatsError> {
        let rules = self
            .rules
            .list_for_account(account_id)
            .await
            .map_err(|e| StatsError::Internal(e.to_string()))?;
        let calendar = match self
            .accounts
            .get_by_id(account_id)
            .await
            .map_err(|e| StatsError::Internal(e.to_string()))?
        {
            Some(a) => self.holidays.for_user(&a.owner_id).await,
            None => Default::default(),
        };

        for (_rule, fee_rule, _horizon) in rules.iter().filter(|r| r.active).filter_map(|r| {
            let fr = r.fee_rule();
            fee_horizon_in(&fr, from, to, &calendar).map(|h| (r, fr, h))
        }) {
            let proj_to = to;

            let metrics = |p: &FeePeriod| -> PeriodMetrics {
                let d_end = p.to.min(proj_to);
                let mut sum: i128 = 0;
                let mut d = p.from;
                while d <= d_end {
                    if let Some(v) = balances.get(&d) {
                        sum += *v as i128;
                    }
                    d += Duration::days(1);
                }
                let n = (d_end - p.from).num_days() + 1;
                let avg = if n > 0 { sum / n as i128 } else { 0 };
                PeriodMetrics {
                    avg_balance_cents: avg.clamp(i64::MIN as i128, i64::MAX as i128) as i64,
                    end_balance_cents: balances.get(&d_end).copied().unwrap_or(0),
                    ..Default::default()
                }
            };

            let fee_rows = generate_fees_in(&fee_rule, from, to, &calendar, metrics);
            for row in fee_rows {
                *per_day.entry(row.date).or_insert(0) += row.amount_cents;
            }
        }

        Ok(())
    }
}

/// Chain the per-day projected amounts into a running-balance series starting
/// from `base_cents` at `from`. Days with no projected amount carry the
/// previous day's balance forward.
fn project_daily<I>(
    per_day: I,
    base_cents: i64,
    from: NaiveDate,
    to: NaiveDate,
) -> Vec<BalancePoint>
where
    I: IntoIterator<Item = (NaiveDate, i64)>,
{
    if from > to {
        return Vec::new();
    }
    let map: BTreeMap<NaiveDate, i64> = per_day.into_iter().collect();
    let mut out = Vec::with_capacity((to - from).num_days() as usize + 1);
    let mut bal = base_cents;
    let mut day = from;
    while day <= to {
        if let Some(a) = map.get(&day) {
            bal += a;
        }
        out.push(BalancePoint {
            date: day,
            balance_cents: bal,
            is_forecast: true,
        });
        day += Duration::days(1);
    }
    out
}

/// Total outflow (positive magnitude) in `cat_id` within `[from, to]`, summed
/// over the supplied rows.
fn spent_in_category(
    rows: &[pfm_domain::Transaction],
    cat_id: &str,
    from: NaiveDate,
    to: NaiveDate,
) -> i64 {
    rows.iter()
        .filter(|t| t.category_id.as_deref() == Some(cat_id))
        .filter(|t| t.cents < 0)
        .filter(|t| from <= t.date && t.date <= to)
        .map(|t| -t.cents)
        .sum()
}
