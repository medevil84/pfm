//! Deposit-interest use-cases: preview the interest accrued over a period and
//! consolidate (credit) it onto the linked current account.
//!
//! The pure engine ([`compute_interest`]) does the day-by-day arithmetic on
//! the effective balance series; this service builds that series from the
//! ledger, enforces access on both the deposit and its backing account, and
//! (on consolidation) persists the interest pair atomically.

use chrono::{Duration, NaiveDate};
use pfm_domain::{
    can_see, can_write, compute_interest, Account, AccountKind, DailyBalance, InterestResult, Role,
    Transaction,
};
use pfm_repo::{
    AccountsRepo, RegisterFilters, SharesRepo, SqlxAccountsRepo, SqlxAuthRepo, SqlxSharesRepo,
    SqlxTransactionsRepo, Storage, TransactionsRepo, UsersRepo,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DepositError {
    #[error("account not found")]
    AccountNotFound,
    #[error("forbidden")]
    Forbidden,
    #[error("account is not a deposit")]
    NotADeposit,
    #[error("deposit has no backing account")]
    MissingBacking,
    #[error("deposit has no rate set")]
    MissingRate,
    #[error("validation error: {0}")]
    Validation(String),
    #[error("internal error: {0}")]
    Internal(String),
}

/// The outcome of a deposit-interest consolidation (credited or previewed).
#[derive(Debug, Clone)]
pub struct DepositInterestOutcome {
    /// Total gross interest accrued over the period, in cents.
    pub gross_cents: i64,
    /// The 26 % withholding, in cents.
    pub withholding_cents: i64,
    /// The net amount credited to the linked current account, in cents.
    pub net_cents: i64,
    /// The first day of the computed period.
    pub from: NaiveDate,
    /// The last day of the computed period.
    pub to: NaiveDate,
    /// The interest pair id, present only when interest was actually written.
    pub pair_id: Option<String>,
}

pub struct DepositService {
    transactions: SqlxTransactionsRepo,
    accounts: SqlxAccountsRepo,
    users: SqlxAuthRepo,
    shares: SqlxSharesRepo,
}

impl DepositService {
    pub fn new(storage: &Storage) -> Self {
        Self {
            transactions: SqlxTransactionsRepo::new(storage),
            accounts: SqlxAccountsRepo::new(storage),
            users: SqlxAuthRepo::new(storage),
            shares: SqlxSharesRepo::new(storage),
        }
    }

    /// Whether `actor_id` may read (`write == false`) or write the account.
    async fn check_access(
        &self,
        actor_id: &str,
        account: &Account,
        write: bool,
    ) -> Result<bool, DepositError> {
        if account.owner_id == actor_id {
            return Ok(true);
        }

        let actor = self
            .users
            .get_by_id(actor_id)
            .await
            .map_err(|e| DepositError::Internal(e.to_string()))?
            .ok_or_else(|| DepositError::Internal("actor not found".into()))?;

        if actor.role == Role::Admin {
            return Ok(true);
        }

        let share = self
            .shares
            .get(&account.id, actor_id)
            .await
            .map_err(|e| DepositError::Internal(e.to_string()))?;

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
    ) -> Result<Account, DepositError> {
        let account = self
            .accounts
            .get_by_id(account_id)
            .await
            .map_err(|e| DepositError::Internal(e.to_string()))?
            .ok_or(DepositError::AccountNotFound)?;

        if !self.check_access(actor_id, &account, write).await? {
            return Err(DepositError::Forbidden);
        }
        Ok(account)
    }

    /// Load the deposit account and its backing account, enforcing write access
    /// on both. Returns `(deposit, primary)`.
    async fn load_deposit_with_backing(
        &self,
        actor_id: &str,
        deposit_id: &str,
    ) -> Result<(Account, Account, i64), DepositError> {
        let deposit = self.load_checked(actor_id, deposit_id, true).await?;
        if deposit.kind != AccountKind::Deposit {
            return Err(DepositError::NotADeposit);
        }
        let Some(primary_id) = deposit.primary_account_id.clone() else {
            return Err(DepositError::MissingBacking);
        };
        let rate = deposit.rate_pct_bps.ok_or(DepositError::MissingRate)?;

        let primary = self.load_checked(actor_id, &primary_id, true).await?;

        Ok((deposit, primary, rate))
    }

    /// Build the per-day closing-balance series for `account` over `[from, to]`.
    ///
    /// The opening balance is the account's `start_balance_cents` plus the sum
    /// of all transactions dated strictly before `from`. Each day's closing
    /// balance is the running total after applying that day's transactions.
    fn daily_balances(
        from: NaiveDate,
        to: NaiveDate,
        rows: &[Transaction],
        opening: i64,
    ) -> Vec<DailyBalance> {
        let mut result = Vec::with_capacity((to - from).num_days() as usize + 1);
        let mut balance = opening;
        let mut tx_idx = 0usize;
        let mut day = from;

        while day <= to {
            while tx_idx < rows.len() && rows[tx_idx].date == day {
                balance += rows[tx_idx].cents;
                tx_idx += 1;
            }
            result.push(DailyBalance {
                date: day,
                balance_cents: balance,
            });
            day += Duration::days(1);
        }

        result
    }

    /// Compute the interest over `[from, to]` for the given deposit account.
    /// Returns the [`InterestResult`] and the daily balance series used.
    async fn compute(
        &self,
        deposit: &Account,
        from: NaiveDate,
        to: NaiveDate,
        rate_bps: i64,
    ) -> Result<InterestResult, DepositError> {
        let effective_from = from.max(deposit.start_date);
        let effective_to = to.max(effective_from);

        let pre_sum = self
            .transactions
            .sum_cents_before(&deposit.id, effective_from)
            .await
            .map_err(|e| DepositError::Internal(e.to_string()))?;
        let opening = deposit.start_balance_cents + pre_sum;

        let rows = self
            .transactions
            .list_for_register(
                &deposit.id,
                effective_from,
                effective_to,
                &RegisterFilters::default(),
            )
            .await
            .map_err(|e| DepositError::Internal(e.to_string()))?;

        let balances = Self::daily_balances(effective_from, effective_to, &rows, opening);
        Ok(compute_interest(&balances, rate_bps))
    }

    /// Preview the interest accrued over `[from, to]` without writing anything.
    pub async fn preview_interest(
        &self,
        actor_id: &str,
        deposit_id: &str,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<InterestResult, DepositError> {
        let (deposit, _primary, rate) =
            self.load_deposit_with_backing(actor_id, deposit_id).await?;
        if from > to {
            return Err(DepositError::Validation("from must be <= to".into()));
        }
        self.compute(&deposit, from, to, rate).await
    }

    /// Compute the interest over `[from, to]` and, when `dry_run` is false and
    /// the net is positive, credit it: the deposit account receives the gross
    /// interest and the linked current account receives the net (after the 26 %
    /// withholding). The two legs are written as a single linked pair, atomically.
    pub async fn consolidate_interest(
        &self,
        actor_id: &str,
        deposit_id: &str,
        from: NaiveDate,
        to: NaiveDate,
        dry_run: bool,
    ) -> Result<DepositInterestOutcome, DepositError> {
        let (deposit, primary, rate) = self.load_deposit_with_backing(actor_id, deposit_id).await?;
        if from > to {
            return Err(DepositError::Validation("from must be <= to".into()));
        }

        let result = self.compute(&deposit, from, to, rate).await?;

        let mut pair_id = None;
        if !dry_run && result.net_cents > 0 {
            let effective_to = to.max(from.max(deposit.start_date));
            let pid = format!(
                "dep:{}:{}:{}",
                deposit.id,
                from.format("%Y-%m-%d"),
                effective_to.format("%Y-%m-%d")
            );
            let desc = Some(format!(
                "deposit interest {} - {}",
                from.format("%Y-%m-%d"),
                effective_to.format("%Y-%m-%d")
            ));
            let _ = self
                .transactions
                .create_deposit_interest_pair(
                    &pid,
                    &deposit.id,
                    &primary.id,
                    effective_to,
                    result.gross_cents,
                    result.net_cents,
                    &desc,
                    actor_id,
                )
                .await
                .map_err(|e| DepositError::Internal(e.to_string()))?;
            pair_id = Some(pid);
        }

        Ok(DepositInterestOutcome {
            gross_cents: result.gross_cents,
            withholding_cents: result.withholding_cents,
            net_cents: result.net_cents,
            from,
            to,
            pair_id,
        })
    }
}
