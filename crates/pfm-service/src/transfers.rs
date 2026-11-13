//! Transfer use-cases: create, get, update, delete, and list transfer pairs.
//! A transfer is two linked transactions sharing a `pair_id`.

use chrono::NaiveDate;
use pfm_domain::{can_see, can_write, Account, AccountKind, Role, Transfer};
use pfm_repo::{
    AccountsRepo, SharesRepo, SqlxAccountsRepo, SqlxAuthRepo, SqlxSharesRepo, SqlxTransactionsRepo,
    Storage, TransactionsRepo, UsersRepo,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TransferError {
    #[error("transfer not found")]
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

/// Input for creating a new transfer.
pub struct TransferCreateCmd {
    pub from_account_id: String,
    pub to_account_id: String,
    pub date: NaiveDate,
    pub cents: i64,
    pub description: Option<String>,
    pub note: Option<String>,
}

/// Input for updating an existing transfer. All fields are `Option`;
/// `None` means "keep the current value".
pub struct TransferUpdateCmd {
    pub from_account_id: Option<String>,
    pub to_account_id: Option<String>,
    pub date: Option<NaiveDate>,
    pub cents: Option<i64>,
    pub description: Option<String>,
    pub note: Option<String>,
}

pub struct TransferService {
    transactions: SqlxTransactionsRepo,
    accounts: SqlxAccountsRepo,
    users: SqlxAuthRepo,
    shares: SqlxSharesRepo,
}

impl TransferService {
    pub fn new(storage: &Storage) -> Self {
        Self {
            transactions: SqlxTransactionsRepo::new(storage),
            accounts: SqlxAccountsRepo::new(storage),
            users: SqlxAuthRepo::new(storage),
            shares: SqlxSharesRepo::new(storage),
        }
    }

    async fn check_access(
        &self,
        actor_id: &str,
        account: &Account,
        write: bool,
    ) -> Result<bool, TransferError> {
        if account.owner_id == actor_id {
            return Ok(true);
        }

        let actor = self
            .users
            .get_by_id(actor_id)
            .await
            .map_err(|e| TransferError::Internal(e.to_string()))?
            .ok_or_else(|| TransferError::Internal("actor not found".into()))?;

        if actor.role == Role::Admin {
            return Ok(true);
        }

        if !write {
            let has_share = self
                .shares
                .get(&account.id, actor_id)
                .await
                .map_err(|e| TransferError::Internal(e.to_string()))?
                .is_some();
            Ok(can_see(actor.role, actor_id, &account.owner_id, has_share))
        } else {
            let share_level = self
                .shares
                .get(&account.id, actor_id)
                .await
                .map_err(|e| TransferError::Internal(e.to_string()))?
                .map(|e| e.level);
            Ok(can_write(
                actor.role,
                actor_id,
                &account.owner_id,
                share_level,
            ))
        }
    }

    async fn get_account_checked(
        &self,
        actor_id: &str,
        account_id: &str,
        write: bool,
    ) -> Result<Account, TransferError> {
        let account = self
            .accounts
            .get_by_id(account_id)
            .await
            .map_err(|e| TransferError::Internal(e.to_string()))?
            .ok_or(TransferError::AccountNotFound)?;

        if !self.check_access(actor_id, &account, write).await? {
            return Err(TransferError::Forbidden);
        }
        Ok(account)
    }

    fn legs_to_transfer(legs: &[pfm_domain::Transaction]) -> Result<Transfer, TransferError> {
        if legs.len() != 2 {
            return Err(TransferError::Internal(format!(
                "expected 2 legs, got {}",
                legs.len()
            )));
        }

        let from_leg = legs
            .iter()
            .find(|t| t.cents < 0)
            .ok_or_else(|| TransferError::Internal("missing from-leg".into()))?;
        let to_leg = legs
            .iter()
            .find(|t| t.cents > 0)
            .ok_or_else(|| TransferError::Internal("missing to-leg".into()))?;

        let pair_id = from_leg
            .pair_id
            .clone()
            .ok_or_else(|| TransferError::Internal("leg missing pair_id".into()))?;

        Ok(Transfer {
            pair_id,
            from_account_id: from_leg.account_id.clone(),
            to_account_id: to_leg.account_id.clone(),
            date: from_leg.date,
            cents: from_leg.cents.abs(),
            description: from_leg.description.clone(),
            note: from_leg.note.clone(),
            created_by: from_leg.created_by.clone(),
            created_at: from_leg.created_at.clone(),
            updated_at: from_leg.updated_at.clone(),
        })
    }

    /// The piggy bank a transfer touches, if any. Both legs of the pair carry
    /// this id so the allocation is hidden from the real balance and from any
    /// ledger that deselects the piggy bank. A transfer between two real
    /// accounts has no piggy bank (`None`).
    fn vacct_of<'a>(from: &'a Account, to: &'a Account) -> Option<&'a str> {
        if to.kind == AccountKind::Virtual {
            Some(to.id.as_str())
        } else if from.kind == AccountKind::Virtual {
            Some(from.id.as_str())
        } else {
            None
        }
    }

    /// Create a new transfer pair. Requires write access on both accounts.
    pub async fn create(
        &self,
        actor_id: &str,
        cmd: &TransferCreateCmd,
    ) -> Result<Transfer, TransferError> {
        if cmd.cents <= 0 {
            return Err(TransferError::Validation("cents must be positive".into()));
        }
        if cmd.from_account_id == cmd.to_account_id {
            return Err(TransferError::Validation(
                "from and to accounts must differ".into(),
            ));
        }

        let from_account = self
            .get_account_checked(actor_id, &cmd.from_account_id, true)
            .await?;
        let to_account = self
            .get_account_checked(actor_id, &cmd.to_account_id, true)
            .await?;

        let pair_id = uuid::Uuid::new_v4().to_string();
        let vacct_id = Self::vacct_of(&from_account, &to_account);

        let (from_tx, _to_tx) = self
            .transactions
            .create_transfer_pair(
                &pair_id,
                &cmd.from_account_id,
                &cmd.to_account_id,
                cmd.date,
                cmd.cents,
                &cmd.description,
                &cmd.note,
                actor_id,
                vacct_id,
            )
            .await
            .map_err(|e| TransferError::Internal(e.to_string()))?;

        Ok(Transfer {
            pair_id: pair_id.clone(),
            from_account_id: from_tx.account_id.clone(),
            to_account_id: cmd.to_account_id.clone(),
            date: from_tx.date,
            cents: cmd.cents,
            description: from_tx.description.clone(),
            note: from_tx.note.clone(),
            created_by: from_tx.created_by.clone(),
            created_at: from_tx.created_at.clone(),
            updated_at: from_tx.updated_at,
        })
    }

    /// Fetch a transfer by pair_id, enforcing read access on both accounts.
    pub async fn get(&self, actor_id: &str, pair_id: &str) -> Result<Transfer, TransferError> {
        let legs = self
            .transactions
            .get_transfer_legs(pair_id)
            .await
            .map_err(|e| TransferError::Internal(e.to_string()))?;

        if legs.is_empty() {
            return Err(TransferError::NotFound);
        }

        let transfer = Self::legs_to_transfer(&legs)?;

        let _from = self
            .get_account_checked(actor_id, &transfer.from_account_id, false)
            .await?;
        let _to = self
            .get_account_checked(actor_id, &transfer.to_account_id, false)
            .await?;

        Ok(transfer)
    }

    /// Update a transfer by deleting both legs and re-inserting.
    /// Requires write access on both the old and new accounts.
    pub async fn update(
        &self,
        actor_id: &str,
        pair_id: &str,
        cmd: &TransferUpdateCmd,
    ) -> Result<Transfer, TransferError> {
        let legs = self
            .transactions
            .get_transfer_legs(pair_id)
            .await
            .map_err(|e| TransferError::Internal(e.to_string()))?;

        if legs.len() != 2 {
            return Err(TransferError::NotFound);
        }

        let existing = Self::legs_to_transfer(&legs)?;

        let new_from = cmd
            .from_account_id
            .clone()
            .unwrap_or(existing.from_account_id.clone());
        let new_to = cmd
            .to_account_id
            .clone()
            .unwrap_or(existing.to_account_id.clone());
        let new_date = cmd.date.unwrap_or(existing.date);
        let new_cents = cmd.cents.unwrap_or(existing.cents);
        let new_description = cmd.description.clone().or(existing.description.clone());
        let new_note = cmd.note.clone().or(existing.note.clone());

        if new_cents <= 0 {
            return Err(TransferError::Validation("cents must be positive".into()));
        }
        if new_from == new_to {
            return Err(TransferError::Validation(
                "from and to accounts must differ".into(),
            ));
        }

        // The transfer is deleted and recreated, so the actor needs write
        // access to the accounts it lives on today as well as to the new ones:
        // checking only the destination would let anyone who knows a
        // `pair_id` move (and thereby destroy) a transfer between accounts
        // they cannot write to.
        self.get_account_checked(actor_id, &existing.from_account_id, true)
            .await?;
        self.get_account_checked(actor_id, &existing.to_account_id, true)
            .await?;
        let from_acc = self.get_account_checked(actor_id, &new_from, true).await?;
        let to_acc = self.get_account_checked(actor_id, &new_to, true).await?;
        let vacct_id = Self::vacct_of(&from_acc, &to_acc);

        self.transactions
            .delete_transfer_pair(pair_id)
            .await
            .map_err(|e| TransferError::Internal(e.to_string()))?;

        let (from_tx, _to_tx) = self
            .transactions
            .create_transfer_pair(
                pair_id,
                &new_from,
                &new_to,
                new_date,
                new_cents,
                &new_description,
                &new_note,
                actor_id,
                vacct_id,
            )
            .await
            .map_err(|e| TransferError::Internal(e.to_string()))?;

        Ok(Transfer {
            pair_id: pair_id.to_string(),
            from_account_id: from_tx.account_id.clone(),
            to_account_id: new_to,
            date: from_tx.date,
            cents: new_cents,
            description: from_tx.description.clone(),
            note: from_tx.note.clone(),
            created_by: from_tx.created_by.clone(),
            created_at: from_tx.created_at.clone(),
            updated_at: from_tx.updated_at,
        })
    }

    /// Delete a transfer pair. Requires write access on both accounts.
    pub async fn delete(&self, actor_id: &str, pair_id: &str) -> Result<(), TransferError> {
        let legs = self
            .transactions
            .get_transfer_legs(pair_id)
            .await
            .map_err(|e| TransferError::Internal(e.to_string()))?;

        if legs.len() != 2 {
            return Err(TransferError::NotFound);
        }

        let transfer = Self::legs_to_transfer(&legs)?;

        let _from = self
            .get_account_checked(actor_id, &transfer.from_account_id, true)
            .await?;
        let _to = self
            .get_account_checked(actor_id, &transfer.to_account_id, true)
            .await?;

        self.transactions
            .delete_transfer_pair(pair_id)
            .await
            .map_err(|e| TransferError::Internal(e.to_string()))?;
        Ok(())
    }

    /// List transfer pairs within a date range, optionally filtered by account.
    pub async fn list(
        &self,
        actor_id: &str,
        from: NaiveDate,
        to: NaiveDate,
        from_account_id: Option<&str>,
        to_account_id: Option<&str>,
    ) -> Result<Vec<Transfer>, TransferError> {
        let legs = self
            .transactions
            .list_transfer_legs(from, to, from_account_id, to_account_id)
            .await
            .map_err(|e| TransferError::Internal(e.to_string()))?;

        let mut pair_map: std::collections::HashMap<String, Vec<pfm_domain::Transaction>> =
            std::collections::HashMap::new();

        for leg in &legs {
            if let Some(pid) = &leg.pair_id {
                // Check read access on the account
                if let Ok(acc) = self
                    .get_account_checked(actor_id, &leg.account_id, false)
                    .await
                {
                    let _ = acc;
                    pair_map.entry(pid.clone()).or_default().push(leg.clone());
                }
            }
        }

        let mut transfers = Vec::new();
        for (_pair_id, group) in pair_map {
            if group.len() == 2 {
                if let Ok(t) = Self::legs_to_transfer(&group) {
                    transfers.push(t);
                }
            }
        }
        transfers.sort_by_key(|a| a.date);
        Ok(transfers)
    }
}
