//! Credit-card settlement use-cases.
//!
//! Two settlement mechanisms:
//!
//! - **Full sweep** (`catch_up_full`): iterates all due cycles from the card's
//!   opening to `as_of`, recomputing each cycle's charge and upserting the
//!   settlement pair. Used by the route handler and the periodic scheduler.
//!
//! - **Delta** (`catch_up_delta`): after a transaction write (insert, update,
//!   delete), recomputes only the cycles affected by the changed dates.
//!   Triggered internally by `TransactionService`.
//!
//! The card's remaining limit is computed as `limit - outstanding`, where
//! `outstanding` is the raw card balance after the last settled date (only
//! cycles not yet billed are counted).

use std::collections::BTreeSet;

use chrono::NaiveDate;
use pfm_domain::{
    all_due_cycles, can_see, can_write, charge_amount, cycle_fee_amount, cycle_for_date,
    next_business_day_in, Account, AccountKind, AccountRule, Cycle, PeriodMetrics, Role,
    Transaction,
};
use pfm_repo::{
    AccountRulesRepo, AccountsRepo, RecurrencesRepo, SharesRepo, SqlxAccountRulesRepo,
    SqlxAccountsRepo, SqlxAuthRepo, SqlxRecurrencesRepo, SqlxSharesRepo, SqlxTransactionsRepo,
    Storage, TransactionsRepo, UsersRepo,
};
use thiserror::Error;

/// The two configurable days that drive a card's settlement cycle. See
/// [`pfm_domain::cycle_for_date`] for the exact semantics.
#[derive(Debug, Clone, Copy)]
pub struct CardCycleDays {
    pub closing_day: i32,
    pub charge_day: i32,
}

#[derive(Debug, Error)]
pub enum CardError {
    #[error("account not found")]
    AccountNotFound,
    #[error("forbidden")]
    Forbidden,
    #[error("account is not a card")]
    NotACard,
    #[error("card has no backing account")]
    MissingBacking,
    #[error("validation error: {0}")]
    Validation(String),
    #[error("internal error: {0}")]
    Internal(String),
}

/// The outcome of a card catch-up operation.
#[derive(Debug, Clone)]
pub struct CardCatchUpOutcome {
    /// The latest charge date that was settled (or would be settled).
    pub latest_charge_date: NaiveDate,
    /// Total number of cycles settled (or that would be settled).
    pub cycles_settled: usize,
    /// Total charge amount across all settled cycles, in cents.
    pub total_charge_cents: i64,
    /// The card's remaining limit in cents, or `None` when the card has no limit.
    pub available_cents: Option<i64>,
    /// Settlement pair ids that were written (empty when dry_run or no charges).
    pub pair_ids: Vec<String>,
}

/// A card cycle that has not been settled yet, with what it is expected to cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardChargeForecast {
    pub card_id: String,
    pub backing_id: String,
    /// Nominal closing date of the cycle.
    pub closing_date: NaiveDate,
    /// Date the card's reset credit will post (last day of the cycle).
    pub card_date: NaiveDate,
    /// Date the backing account will be debited (business-day adjusted).
    pub backing_date: NaiveDate,
    /// Net spend of the cycle on the card, in cents (positive).
    pub spend_cents: i64,
    /// Rule-defined oneri added to the backing debit, in cents.
    pub fee_cents: i64,
}

pub struct CardService {
    recs: SqlxRecurrencesRepo,
    recurrences: crate::recurrences::RecurrenceService,
    transactions: SqlxTransactionsRepo,
    accounts: SqlxAccountsRepo,
    rules: SqlxAccountRulesRepo,
    users: SqlxAuthRepo,
    shares: SqlxSharesRepo,
    holidays: crate::holidays::HolidayService,
}

impl CardService {
    pub fn new(storage: &Storage) -> Self {
        Self {
            recs: SqlxRecurrencesRepo::new(storage),
            recurrences: crate::recurrences::RecurrenceService::new(storage),
            transactions: SqlxTransactionsRepo::new(storage),
            accounts: SqlxAccountsRepo::new(storage),
            rules: SqlxAccountRulesRepo::new(storage),
            users: SqlxAuthRepo::new(storage),
            holidays: crate::holidays::HolidayService::new(storage),
            shares: SqlxSharesRepo::new(storage),
        }
    }

    /// Settle a single cycle: credit the card its real spend on the closing
    /// date, and debit the backing account that spend plus any rule-defined
    /// oneri (bollo, canoni) on the working-day-adjusted charge date.
    ///
    /// Returns `(backing_total_cents, pair_id_if_written)`. A cycle that nets to
    /// zero spend writes nothing and removes any prior settlement pair.
    async fn settle_cycle(
        &self,
        card_id: &str,
        backing_id: &str,
        cycle: &Cycle,
        rules: &[AccountRule],
        dry_run: bool,
    ) -> Result<(i64, Option<String>), CardError> {
        // Keyed by the charge month, not the exact date, so changing the charge
        // day updates the existing settlement instead of stacking a new one.
        let pid = format!("cc:{}:{}", card_id, cycle.charge_date.format("%Y-%m"));

        let raw = self
            .transactions
            .list_card_cycle(card_id, cycle.window_start(), cycle.window_end())
            .await
            .map_err(|e| CardError::Internal(e.to_string()))?;
        let spend = charge_amount(&raw);

        if spend <= 0 {
            if !dry_run {
                self.transactions
                    .delete_card_charge_pair(&pid)
                    .await
                    .map_err(|e| CardError::Internal(e.to_string()))?;
            }
            return Ok((0, None));
        }

        // Rule-defined oneri for this cycle go only onto the backing debit; the
        // card's reset credit stays equal to the real spend.
        let backing_total = spend + cycle_oneri(rules, cycle, &raw, spend);

        if !dry_run {
            let owner = self
                .accounts
                .get_by_id(card_id)
                .await
                .map_err(|e| CardError::Internal(e.to_string()))?
                .map(|a| a.owner_id);
            let backing_date = self.backing_date(owner.as_deref(), cycle.charge_date).await;
            // The card's reset credit is dated on the last day of the cycle (the
            // closing date, or the Friday before when it falls on a weekend), so the
            // statement always shows in the month it closes. A weekend closing day
            // would otherwise push the credit into the next cycle's window.
            let card_date = cycle.window_end();
            self.realign_card_leg_date(&pid, card_id, cycle.closing_date, card_date)
                .await?;
            // The card is reset at closing, but the backing account is debited
            // only on the charge date: until then the debit stays a forecast.
            if backing_date > chrono::Utc::now().date_naive() {
                self.transactions
                    .upsert_card_reset_leg(
                        &pid,
                        card_id,
                        card_date,
                        spend,
                        &Some("card settlement".into()),
                        "system",
                    )
                    .await
                    .map_err(|e| CardError::Internal(e.to_string()))?;
                return Ok((backing_total, Some(pid)));
            }
            self.transactions
                .create_card_charge_pair(
                    &pid,
                    backing_id,
                    card_id,
                    card_date,
                    backing_date,
                    spend,
                    backing_total,
                    &Some("card settlement".into()),
                    "system",
                )
                .await
                .map_err(|e| CardError::Internal(e.to_string()))?;
        }
        Ok((backing_total, Some(pid)))
    }

    /// The date the backing account is debited for a nominal `charge_date`: the
    /// first business day of the card owner's holiday calendar on or after it.
    async fn backing_date(&self, owner_id: Option<&str>, charge_date: NaiveDate) -> NaiveDate {
        let calendar = match owner_id {
            Some(o) => self.holidays.for_user(o).await,
            None => Default::default(),
        };
        next_business_day_in(&calendar, charge_date)
    }

    /// Forecast of the not-yet-settled cycles of every card `actor_id` can see,
    /// keeping those whose card credit or backing debit falls in `[from, to]`
    /// after `today`.
    pub async fn forecast(
        &self,
        actor_id: &str,
        from: NaiveDate,
        to: NaiveDate,
        today: NaiveDate,
    ) -> Result<Vec<CardChargeForecast>, CardError> {
        let is_admin = self
            .users
            .get_by_id(actor_id)
            .await
            .map_err(|e| CardError::Internal(e.to_string()))?
            .is_some_and(|u| u.role == Role::Admin);
        let accounts = self
            .accounts
            .list_visible(actor_id, is_admin)
            .await
            .map_err(|e| CardError::Internal(e.to_string()))?;
        let mut out = Vec::new();
        for card in accounts.iter().filter(|a| a.kind == AccountKind::Card) {
            out.extend(self.forecast_for_card(card, from, to, today).await?);
        }
        out.sort_by_key(|f| (f.backing_date, f.card_id.clone()));
        Ok(out)
    }

    /// Forecast for one card, without permission checks (callers have already
    /// authorised access to the card or its backing account).
    pub async fn forecast_for_card(
        &self,
        card: &Account,
        from: NaiveDate,
        to: NaiveDate,
        today: NaiveDate,
    ) -> Result<Vec<CardChargeForecast>, CardError> {
        let internal = |e: &dyn std::fmt::Display| CardError::Internal(e.to_string());
        let (Some(charge_day), Some(backing)) = (card.charge_day, card.primary_account_id.clone())
        else {
            return Ok(Vec::new());
        };
        let closing_day = card.closing_day.unwrap_or(charge_day);
        if !(1..=28).contains(&closing_day) || !(1..=28).contains(&charge_day) {
            return Ok(Vec::new());
        }
        let effective_start = match self
            .transactions
            .min_raw_date(&card.id)
            .await
            .map_err(|e| internal(&e))?
        {
            Some(min_date) => card.start_date.min(min_date),
            None => card.start_date,
        };
        let rules = self
            .rules
            .list_for_account(&card.id)
            .await
            .map_err(|e| internal(&e))?;
        let recs: Vec<_> = self
            .recs
            .list_for_account(&card.id)
            .await
            .map_err(|e| internal(&e))?
            .into_iter()
            .filter(|r| r.active && r.linked_account_id.is_none())
            .collect();

        let mut out = Vec::new();
        for cycle in all_due_cycles(closing_day, charge_day, to, effective_start) {
            let backing_date = self
                .backing_date(Some(&card.owner_id), cycle.charge_date)
                .await;
            let card_date = cycle.window_end();
            let in_range = |d: NaiveDate| d >= from && d <= to;
            if !(in_range(backing_date) || in_range(card_date))
                || (backing_date <= today && card_date <= today)
            {
                continue;
            }
            // A settlement that already exists is a real row, shown as such.
            let pid = format!("cc:{}:{}", card.id, cycle.charge_date.format("%Y-%m"));
            // Only a debit already booked on the backing account replaces it; a
            // card-only reset leaves the debit as the forecast.
            if self
                .transactions
                .get_transfer_legs(&pid)
                .await
                .map_err(|e| internal(&e))?
                .iter()
                .any(|l| l.account_id == backing)
            {
                continue;
            }

            // Spend booked so far in the cycle, plus the card's recurrences that are
            // still to come (those up to today are already booked rows).
            let mut rows = self
                .transactions
                .list_card_cycle(&card.id, cycle.window_start(), cycle.window_end())
                .await
                .map_err(|e| internal(&e))?;
            let booked: BTreeSet<(String, NaiveDate)> = rows
                .iter()
                .filter_map(|t| t.rec_id.clone().map(|r| (r, t.date)))
                .collect();
            let occ_from = (cycle.window_start() + chrono::Duration::days(1))
                .max(today + chrono::Duration::days(1));
            if occ_from <= cycle.window_end() {
                for rec in &recs {
                    let occs = self
                        .recurrences
                        .occurrences(&card.owner_id, &rec.id, occ_from, cycle.window_end())
                        .await
                        .map_err(|e| internal(&e))?;
                    for o in occs {
                        if booked.contains(&(rec.id.clone(), o.date)) {
                            continue;
                        }
                        rows.push(forecast_row(&card.id, &rec.id, o.date, o.amount_cents));
                    }
                }
            }
            let spend = charge_amount(&rows);
            if spend <= 0 {
                continue;
            }
            out.push(CardChargeForecast {
                card_id: card.id.clone(),
                backing_id: backing.clone(),
                closing_date: cycle.closing_date,
                card_date,
                backing_date,
                spend_cents: spend,
                fee_cents: cycle_oneri(&rules, &cycle, &rows, spend),
            });
        }
        Ok(out)
    }

    /// Settlements written before the weekend rule were dated on the nominal
    /// closing date. When that date is a weekend, move the card leg to the cycle's
    /// last day. A leg the user has dated elsewhere is left alone.
    async fn realign_card_leg_date(
        &self,
        pair_id: &str,
        card_id: &str,
        nominal_closing: NaiveDate,
        card_date: NaiveDate,
    ) -> Result<(), CardError> {
        if nominal_closing == card_date {
            return Ok(());
        }
        let internal = |e: &dyn std::fmt::Display| CardError::Internal(e.to_string());
        let legs = self
            .transactions
            .get_transfer_legs(pair_id)
            .await
            .map_err(|e| internal(&e))?;
        if let Some(mut leg) = legs
            .into_iter()
            .find(|l| l.account_id == card_id && l.date == nominal_closing)
        {
            leg.date = card_date;
            self.transactions
                .update(&leg)
                .await
                .map_err(|e| internal(&e))?;
        }
        Ok(())
    }

    /// Whether `actor_id` may read (`write == false`) or write the account.
    async fn check_access(
        &self,
        actor_id: &str,
        account: &Account,
        write: bool,
    ) -> Result<bool, CardError> {
        if account.owner_id == actor_id {
            return Ok(true);
        }

        let actor = self
            .users
            .get_by_id(actor_id)
            .await
            .map_err(|e| CardError::Internal(e.to_string()))?
            .ok_or_else(|| CardError::Internal("actor not found".into()))?;

        if actor.role == Role::Admin {
            return Ok(true);
        }

        let share = self
            .shares
            .get(&account.id, actor_id)
            .await
            .map_err(|e| CardError::Internal(e.to_string()))?;

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

    /// Load an account by id and enforce the actor's access.
    async fn load_checked(
        &self,
        actor_id: &str,
        account_id: &str,
        write: bool,
    ) -> Result<Account, CardError> {
        let account = self
            .accounts
            .get_by_id(account_id)
            .await
            .map_err(|e| CardError::Internal(e.to_string()))?
            .ok_or(CardError::AccountNotFound)?;

        if !self.check_access(actor_id, &account, write).await? {
            return Err(CardError::Forbidden);
        }
        Ok(account)
    }

    /// The remaining limit of a card as seen by `actor_id`, or `None` when the
    /// account is not a card or has no limit. Settlement credits dated at the
    /// accounting closing day are ignored, so the figure keeps moving with the
    /// spend that has not been charged to the backing account yet.
    pub async fn available_for(
        &self,
        actor_id: &str,
        account_id: &str,
    ) -> Result<Option<i64>, CardError> {
        Ok(self.outstanding_for(actor_id, account_id).await?.0)
    }

    /// `(available, outstanding)` of a card, both `None` for non-card accounts;
    /// `available` is also `None` when the card has no limit. Movements dated
    /// after today are not counted: they are not yet part of the balance.
    pub async fn outstanding_for(
        &self,
        actor_id: &str,
        account_id: &str,
    ) -> Result<(Option<i64>, Option<i64>), CardError> {
        let card = self.load_checked(actor_id, account_id, false).await?;
        if card.kind != AccountKind::Card {
            return Ok((None, None));
        }
        let raw = self
            .transactions
            .sum_raw_after(
                &card.id,
                card.last_settled_date,
                chrono::Utc::now().date_naive(),
            )
            .await
            .map_err(|e| CardError::Internal(e.to_string()))?;
        let outstanding = (card.start_balance_cents + raw).min(0).abs();
        Ok((card.limit_cents.map(|l| l - outstanding), Some(outstanding)))
    }

    /// Compute the card's available limit (remaining credit).
    ///
    /// `outstanding = |min(0, start_balance + raw_cents_after(last_settled))|`.
    /// Only raw (non-settlement) movements after the last settled date are
    /// counted: settlement legs (which credit the card) are excluded by
    /// filtering `pair_id IS NULL`.
    async fn compute_available(&self, card: &Account) -> Result<Option<i64>, CardError> {
        let Some(limit) = card.limit_cents else {
            return Ok(None);
        };
        let raw_outstanding = self
            .transactions
            .sum_raw_after(
                &card.id,
                card.last_settled_date,
                chrono::Utc::now().date_naive(),
            )
            .await
            .map_err(|e| CardError::Internal(e.to_string()))?;
        let total_balance = card.start_balance_cents + raw_outstanding;
        let outstanding = total_balance.min(0).abs();
        Ok(Some(limit - outstanding))
    }

    /// Full sweep: settle all due cycles for the card from its opening to `as_of`.
    ///
    /// When `dry_run` is false, settlement pairs are written and the
    /// `last_settled_date` watermark is advanced. Returns a summary of what
    /// was (or would be) settled.
    pub async fn catch_up_full(
        &self,
        actor_id: &str,
        card_account_id: &str,
        as_of: NaiveDate,
        dry_run: bool,
    ) -> Result<CardCatchUpOutcome, CardError> {
        let card = self.load_checked(actor_id, card_account_id, true).await?;

        // The settlement also moves money on the backing account: require write
        // access on it.
        if let Some(ref backing) = card.primary_account_id {
            let _backing_acc = self.load_checked(actor_id, backing, true).await?;
        }

        self._catch_up_internal(&card, as_of, dry_run).await
    }

    /// Full sweep for all active cards: used by the periodic scheduler.
    /// For each active card account, runs a catch-up as of `as_of`.
    /// Errors on individual cards are logged but do not stop the sweep.
    pub async fn catch_up_all(&self, as_of: NaiveDate) -> usize {
        let cards = match self.accounts.list_active_cards().await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("card catch-up: failed to list cards: {e}");
                return 0;
            }
        };

        let mut settled = 0;
        for card in &cards {
            if card.charge_day.is_none() || card.primary_account_id.is_none() {
                continue;
            }
            let result = self._catch_up_internal(card, as_of, false).await;
            match result {
                Ok(outcome) => {
                    settled += 1;
                    tracing::debug!(
                        "card catch-up: {} settled {} cycles, total charge {}",
                        card.name,
                        outcome.cycles_settled,
                        outcome.total_charge_cents
                    );
                }
                Err(e) => {
                    tracing::warn!("card catch-up: {} ({}): {}", card.name, card.id, e);
                }
            }
        }
        settled
    }

    /// Delta settlement: recompute only the cycles affected by transaction
    /// writes on the card. Called internally by `TransactionService` after
    /// insert/update/delete. No permission check (the write already verified access).
    pub async fn settle_delta(
        &self,
        card_id: &str,
        days: CardCycleDays,
        card_start_date: NaiveDate,
        card_last_settled: NaiveDate,
        backing_id: &str,
        affected_dates: &[NaiveDate],
    ) -> Result<(), CardError> {
        let closing_day = days.closing_day;
        let charge_day = days.charge_day;
        // Use the earliest raw transaction as effective start so that
        // backfilled/older transactions are correctly mapped to their cycle.
        let effective_start = match self
            .transactions
            .min_raw_date(card_id)
            .await
            .map_err(|e| CardError::Internal(e.to_string()))?
        {
            Some(min_date) => card_start_date.min(min_date),
            None => card_start_date,
        };

        // The card's own rules define the oneri (bollo, canoni) added to each
        // cycle's backing debit.
        let rules = self
            .rules
            .list_for_account(card_id)
            .await
            .map_err(|e| CardError::Internal(e.to_string()))?;

        // Determine which unique cycles are affected (deduplicated by closing
        // date, which uniquely identifies a cycle).
        let mut affected_cycles: BTreeSet<(NaiveDate, NaiveDate, NaiveDate)> = BTreeSet::new();
        for &date in affected_dates {
            if let Some(cycle) = cycle_for_date(closing_day, charge_day, date, effective_start) {
                // Only settle cycles that are already due (charge_date <= last_settled
                // or the cycle is in the past). Future cycles will be handled by
                // the next full sweep.
                if cycle.charge_date <= card_last_settled
                    || cycle.window_end() <= chrono::Utc::now().date_naive()
                {
                    affected_cycles.insert((
                        cycle.cycle_start,
                        cycle.closing_date,
                        cycle.charge_date,
                    ));
                }
            }
        }

        for (cycle_start, closing_date, charge_date) in affected_cycles {
            let cycle = Cycle {
                cycle_start,
                closing_date,
                charge_date,
            };
            self.settle_cycle(card_id, backing_id, &cycle, &rules, false)
                .await?;
        }

        Ok(())
    }

    /// The end-of-cycle settlement for `card_account_id`, as of `as_of`.
    /// Convenience wrapper around `catch_up_full` with permission checks.
    pub async fn charge_cycle(
        &self,
        actor_id: &str,
        card_account_id: &str,
        as_of: NaiveDate,
        dry_run: bool,
    ) -> Result<CardCatchUpOutcome, CardError> {
        self.catch_up_full(actor_id, card_account_id, as_of, dry_run)
            .await
    }

    /// Internal full sweep without permission checks. Used by the scheduler.
    async fn _catch_up_internal(
        &self,
        card: &Account,
        as_of: NaiveDate,
        dry_run: bool,
    ) -> Result<CardCatchUpOutcome, CardError> {
        if card.kind != AccountKind::Card {
            return Err(CardError::NotACard);
        }
        let Some(backing) = card.primary_account_id.clone() else {
            return Err(CardError::MissingBacking);
        };

        let charge_day = card
            .charge_day
            .ok_or_else(|| CardError::Validation("card has no charge_day set".into()))?;
        if !(1..=28).contains(&charge_day) {
            return Err(CardError::Validation(
                "charge_day must be between 1 and 28".into(),
            ));
        }
        // The closing day defaults to the charge day for cards migrated
        // before the closing/charge split existed, reproducing their
        // previous single-day behavior exactly.
        let closing_day = card.closing_day.unwrap_or(charge_day);
        if !(1..=28).contains(&closing_day) {
            return Err(CardError::Validation(
                "closing_day must be between 1 and 28".into(),
            ));
        }

        // Use the earliest raw transaction as an effective start date so that
        // imported/backfilled transactions are picked up even if the stored
        // start_date is later than the oldest spend.
        let effective_start = match self
            .transactions
            .min_raw_date(&card.id)
            .await
            .map_err(|e| CardError::Internal(e.to_string()))?
        {
            Some(min_date) => card.start_date.min(min_date),
            None => card.start_date,
        };

        let cycles = all_due_cycles(closing_day, charge_day, as_of, effective_start);
        if cycles.is_empty() {
            // No billing cycle has completed yet. Return a zero-charge success
            // so the client can show a friendly "nothing to settle" message
            // instead of an HTTP 422.
            let available_cents = self.compute_available(card).await?;
            return Ok(CardCatchUpOutcome {
                latest_charge_date: card.last_settled_date,
                cycles_settled: 0,
                total_charge_cents: 0,
                available_cents,
                pair_ids: Vec::new(),
            });
        }

        // The card's own rules define the oneri (bollo, canoni) added to each
        // cycle's backing debit.
        let rules = self
            .rules
            .list_for_account(&card.id)
            .await
            .map_err(|e| CardError::Internal(e.to_string()))?;

        let mut total_charge: i64 = 0;
        let mut pair_ids = Vec::new();
        let latest_charge_date = cycles.last().map(|c| c.charge_date).unwrap();
        // The watermark is the card's last reset (the settled cycle's closing
        // date, weekend-adjusted): the spend after it is what the card still owes.
        // It must not be the charge date, which may lie in the future.
        let watermark = cycles.last().map(|c| c.window_end()).unwrap();

        for cycle in &cycles {
            let (backing_total, pid) = self
                .settle_cycle(&card.id, &backing, cycle, &rules, dry_run)
                .await?;
            total_charge += backing_total;
            if let Some(pid) = pid {
                if !dry_run {
                    pair_ids.push(pid);
                }
            }
        }

        if !dry_run {
            self.accounts
                .update_last_settled(&card.id, watermark)
                .await
                .map_err(|e| CardError::Internal(e.to_string()))?;
        }

        let available_cents = if dry_run {
            // Dry-run: watermark is not advanced; use the current value.
            self.compute_available(card).await?
        } else {
            // Real settlement: use the new watermark.
            let updated_card = Account {
                last_settled_date: watermark,
                ..card.clone()
            };
            self.compute_available(&updated_card).await?
        };

        Ok(CardCatchUpOutcome {
            latest_charge_date,
            cycles_settled: cycles.len(),
            total_charge_cents: total_charge,
            available_cents,
            pair_ids,
        })
    }
}

/// Metrics of one card cycle, expressed as positive magnitudes so a rule's
/// threshold compares against the statement's size (e.g. `end_balance > 77.47€`
/// fires on a €2131.81 statement). `spend` is the cycle's net owed amount
/// ([`charge_amount`]). A card cycle opens at zero exposure right after the prior
/// settlement, so `end_balance` and `avg_balance` are the owed amount; per-day
/// averaging is not needed for the fixed/end-balance rules cards use.
/// Rule-defined oneri (bollo, canoni) of a cycle with the given rows and net spend.
fn cycle_oneri(rules: &[AccountRule], cycle: &Cycle, rows: &[Transaction], spend: i64) -> i64 {
    let metrics = cycle_metrics(rows, spend);
    rules
        .iter()
        .filter(|r| r.active)
        .map(|r| cycle_fee_amount(&r.fee_rule(), cycle.closing_date, &metrics))
        .sum()
}

/// A not-yet-booked card movement standing in for a recurrence occurrence.
fn forecast_row(card_id: &str, rec_id: &str, date: NaiveDate, cents: i64) -> Transaction {
    Transaction {
        id: String::new(),
        client_id: format!("forecast:{rec_id}:{date}"),
        account_id: card_id.to_string(),
        date,
        cents,
        description: None,
        category_id: None,
        note: None,
        pair_id: None,
        rec_id: Some(rec_id.to_string()),
        rule_id: None,
        override_: None,
        computed: None,
        vacct_id: None,
        import_hash: None,
        created_by: String::new(),
        created_at: String::new(),
        updated_at: String::new(),
    }
}

fn cycle_metrics(rows: &[Transaction], spend: i64) -> PeriodMetrics {
    let mut total_out = 0i64;
    let mut total_in = 0i64;
    for t in rows {
        if t.cents < 0 {
            total_out += -t.cents;
        } else {
            total_in += t.cents;
        }
    }
    PeriodMetrics {
        avg_balance_cents: spend,
        end_balance_cents: spend,
        total_out_cents: total_out,
        total_in_cents: total_in,
        tx_count: rows.len() as i64,
    }
}
