//! Bulk operations: delete or recategorize multiple transactions atomically.
//! All-or-nothing permission check: if ANY id in the selection lacks write
//! permission, the entire operation is rejected.

use pfm_domain::{can_write, Account, Role};
use pfm_repo::{
    AccountsRepo, SharesRepo, SqlxAccountsRepo, SqlxAuthRepo, SqlxSharesRepo, SqlxTransactionsRepo,
    Storage, TransactionsRepo, UsersRepo,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BulkError {
    #[error("forbidden")]
    Forbidden,
    #[error("validation error: {0}")]
    Validation(String),
    #[error("internal error: {0}")]
    Internal(String),
}

/// The action to perform on a bulk selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BulkAction {
    Delete,
    Recategorize,
}

pub struct BulkService {
    transactions: SqlxTransactionsRepo,
    accounts: SqlxAccountsRepo,
    users: SqlxAuthRepo,
    shares: SqlxSharesRepo,
    category_access: crate::CategoryAccess,
}

impl BulkService {
    pub fn new(storage: &Storage) -> Self {
        Self {
            transactions: SqlxTransactionsRepo::new(storage),
            accounts: SqlxAccountsRepo::new(storage),
            users: SqlxAuthRepo::new(storage),
            shares: SqlxSharesRepo::new(storage),
            category_access: crate::CategoryAccess::new(storage),
        }
    }

    async fn check_write_access(
        &self,
        actor_id: &str,
        account: &Account,
    ) -> Result<bool, BulkError> {
        if account.owner_id == actor_id {
            return Ok(true);
        }

        let actor = self
            .users
            .get_by_id(actor_id)
            .await
            .map_err(|e| BulkError::Internal(e.to_string()))?
            .ok_or_else(|| BulkError::Internal("actor not found".into()))?;

        if actor.role == Role::Admin {
            return Ok(true);
        }

        let share_level = self
            .shares
            .get(&account.id, actor_id)
            .await
            .map_err(|e| BulkError::Internal(e.to_string()))?
            .map(|e| e.level);

        Ok(can_write(
            actor.role,
            actor_id,
            &account.owner_id,
            share_level,
        ))
    }

    /// Atomically delete multiple transactions. If any id in the selection
    /// belongs to an account the actor cannot write to, the entire operation
    /// is rejected (no partial execution).
    pub async fn bulk_delete(&self, actor_id: &str, ids: &[String]) -> Result<u64, BulkError> {
        if ids.is_empty() {
            return Err(BulkError::Validation("no ids provided".into()));
        }

        for id in ids {
            let tx = self
                .transactions
                .get_by_id(id)
                .await
                .map_err(|e| BulkError::Internal(e.to_string()))?;

            let Some(tx) = tx else {
                continue;
            };

            let account = self
                .accounts
                .get_by_id(&tx.account_id)
                .await
                .map_err(|e| BulkError::Internal(e.to_string()))?
                .ok_or_else(|| BulkError::Internal("account not found".into()))?;

            if !self.check_write_access(actor_id, &account).await? {
                return Err(BulkError::Forbidden);
            }
        }

        self.transactions
            .bulk_delete(ids)
            .await
            .map_err(|e| BulkError::Internal(e.to_string()))
    }

    /// Atomically recategorize multiple transactions. If any id in the
    /// selection belongs to an account the actor cannot write to, the entire
    /// operation is rejected.
    pub async fn bulk_recategorize(
        &self,
        actor_id: &str,
        ids: &[String],
        category_id: Option<&str>,
    ) -> Result<u64, BulkError> {
        if ids.is_empty() {
            return Err(BulkError::Validation("no ids provided".into()));
        }

        if let Some(cat) = category_id {
            self.category_access
                .ensure_usable(actor_id, cat)
                .await
                .map_err(|e| match e {
                    crate::CategoryAccessError::NotFound => {
                        BulkError::Validation("unknown category".into())
                    }
                    crate::CategoryAccessError::Internal(m) => BulkError::Internal(m),
                })?;
        }

        for id in ids {
            let tx = self
                .transactions
                .get_by_id(id)
                .await
                .map_err(|e| BulkError::Internal(e.to_string()))?;

            let Some(tx) = tx else {
                continue;
            };

            let account = self
                .accounts
                .get_by_id(&tx.account_id)
                .await
                .map_err(|e| BulkError::Internal(e.to_string()))?
                .ok_or_else(|| BulkError::Internal("account not found".into()))?;

            if !self.check_write_access(actor_id, &account).await? {
                return Err(BulkError::Forbidden);
            }
        }

        self.transactions
            .bulk_recategorize(ids, category_id)
            .await
            .map_err(|e| BulkError::Internal(e.to_string()))
    }
}
