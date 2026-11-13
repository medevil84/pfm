//! Virtual-account (piggy-bank) balance use-cases.
//!
//! A piggy bank is a *slice* of a real account: an account with
//! `kind = Virtual` whose `primary_account_id` points at the real account it
//! represents. Setting money aside is a pair of linked movements (a transfer)
//! that stamp both legs with the piggy bank's id (`vacct_id`). The real
//! account therefore exposes two balances:
//!
//!   * **real** — what the bank shows; every set-aside is ignored
//!     (`vacct_id IS NULL`).
//!   * **allocated** — what is genuinely available once the piggy banks the
//!     user chose to include are folded back
//!     (`vacct_id IS NULL OR vacct_id IN (selected)`).
//!
//! Piggy banks are never added on top of their real account when computing the
//! overall net worth: their total is already contained in the real account's
//! balance, so summing them separately would double-count.
//!
//! This service enforces read access on the account and delegates the
//! partitioning to the pure [`virtual_balances`] engine.

use pfm_domain::{can_see, can_write, virtual_balances, Account, Role};
use pfm_repo::{
    AccountsRepo, SharesRepo, SqlxAccountsRepo, SqlxAuthRepo, SqlxSharesRepo, SqlxTransactionsRepo,
    Storage, TransactionsRepo, UsersRepo,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum VirtualError {
    #[error("account not found")]
    AccountNotFound,
    #[error("forbidden")]
    Forbidden,
    #[error("internal error: {0}")]
    Internal(String),
}

/// The two balances of an account that feeds one or more piggy banks.
#[derive(Debug, Clone)]
pub struct VirtualBalanceView {
    /// The account the balances belong to.
    pub account_id: String,
    /// Bank balance (opening balance plus every non-allocated movement).
    pub real_cents: i64,
    /// Available balance (non-allocated movements plus the allocation legs of
    /// the selected piggy banks). With no piggy selected this equals
    /// `real_cents`.
    pub allocated_cents: i64,
}

pub struct VirtualAccountService {
    transactions: SqlxTransactionsRepo,
    accounts: SqlxAccountsRepo,
    users: SqlxAuthRepo,
    shares: SqlxSharesRepo,
}

impl VirtualAccountService {
    pub fn new(storage: &Storage) -> Self {
        Self {
            transactions: SqlxTransactionsRepo::new(storage),
            accounts: SqlxAccountsRepo::new(storage),
            users: SqlxAuthRepo::new(storage),
            shares: SqlxSharesRepo::new(storage),
        }
    }

    /// Whether `actor_id` may read the account.
    async fn check_access(
        &self,
        actor_id: &str,
        account: &Account,
        write: bool,
    ) -> Result<bool, VirtualError> {
        if account.owner_id == actor_id {
            return Ok(true);
        }

        let actor = self
            .users
            .get_by_id(actor_id)
            .await
            .map_err(|e| VirtualError::Internal(e.to_string()))?
            .ok_or_else(|| VirtualError::Internal("actor not found".into()))?;

        if actor.role == Role::Admin {
            return Ok(true);
        }

        let share = self
            .shares
            .get(&account.id, actor_id)
            .await
            .map_err(|e| VirtualError::Internal(e.to_string()))?;

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

    /// Compute the two balances of `account_id` for the actor, folding back the
    /// allocations of the piggy banks in `selected`. Enforces read access on
    /// the account.
    ///
    /// `selected` is the set of piggy-bank ids the user wants included in the
    /// "available" balance. An empty selection yields `allocated_cents ==
    /// real_cents`. Ids that are not piggy banks of this account (or do not
    /// exist) simply match no allocation leg and are harmless.
    pub async fn balance(
        &self,
        actor_id: &str,
        account_id: &str,
        selected: &[String],
    ) -> Result<VirtualBalanceView, VirtualError> {
        let account = self
            .accounts
            .get_by_id(account_id)
            .await
            .map_err(|e| VirtualError::Internal(e.to_string()))?
            .ok_or(VirtualError::AccountNotFound)?;

        if !self.check_access(actor_id, &account, false).await? {
            return Err(VirtualError::Forbidden);
        }

        // A piggy bank's own set-aside legs are always part of its balance, even
        // when it is deselected elsewhere (its `vacct_id` equals its own id).
        // Fold the account's own id into the selection so the engine counts them.
        let mut selected = selected.to_vec();
        if account.kind == pfm_domain::AccountKind::Virtual
            && !selected.iter().any(|s| s == &account.id)
        {
            selected.push(account.id.clone());
        }

        let rows = self
            .transactions
            .list_all_rows(&account.id)
            .await
            .map_err(|e| VirtualError::Internal(e.to_string()))?;

        let balances = virtual_balances(account.start_balance_cents, &rows, &selected);

        Ok(VirtualBalanceView {
            account_id: account.id,
            real_cents: balances.real_cents,
            allocated_cents: balances.allocated_cents,
        })
    }
}
