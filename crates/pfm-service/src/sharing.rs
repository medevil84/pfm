//! Account sharing use-cases: grant, revoke, search, and list shares.

use pfm_domain::{Role, ShareEntry, ShareLevel};
use pfm_repo::{
    AccountsRepo, AuditRepo, SharesRepo, SqlxAccountsRepo, SqlxAuthRepo, SqlxSharesRepo, Storage,
    UsersRepo,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ShareError {
    #[error("account not found")]
    AccountNotFound,
    #[error("user not found")]
    UserNotFound,
    #[error("forbidden")]
    Forbidden,
    #[error("validation error: {0}")]
    Validation(String),
    #[error("internal error: {0}")]
    Internal(String),
}

/// Manages account shares: granting and revoking access, searching by email,
/// and listing shares.
pub struct SharingService {
    shares: SqlxSharesRepo,
    users: SqlxAuthRepo,
    accounts: SqlxAccountsRepo,
}

impl SharingService {
    pub fn new(storage: &Storage) -> Self {
        Self {
            shares: SqlxSharesRepo::new(storage),
            users: SqlxAuthRepo::new(storage),
            accounts: SqlxAccountsRepo::new(storage),
        }
    }

    async fn check_owner_or_admin(
        &self,
        actor_id: &str,
        account_id: &str,
    ) -> Result<bool, ShareError> {
        let account = self
            .accounts
            .get_by_id(account_id)
            .await
            .map_err(|e| ShareError::Internal(e.to_string()))?
            .ok_or(ShareError::AccountNotFound)?;

        if account.owner_id == actor_id {
            return Ok(true);
        }

        let actor = self
            .users
            .get_by_id(actor_id)
            .await
            .map_err(|e| ShareError::Internal(e.to_string()))?
            .ok_or(ShareError::Internal("actor not found".into()))?;

        Ok(actor.role == Role::Admin)
    }

    /// Grant (or update) a share on `account_id` for the user identified by
    /// `target_email`. Only the account owner or an admin may call this.
    /// Returns the persisted share entry.
    pub async fn add_share(
        &self,
        actor_id: &str,
        account_id: &str,
        target_email: &str,
        level: ShareLevel,
    ) -> Result<ShareEntry, ShareError> {
        if !self.check_owner_or_admin(actor_id, account_id).await? {
            return Err(ShareError::Forbidden);
        }

        // Fetch the account to get the owner_id (avoid sharing with self).
        let account = self
            .accounts
            .get_by_id(account_id)
            .await
            .map_err(|e| ShareError::Internal(e.to_string()))?
            .ok_or(ShareError::AccountNotFound)?;

        let target = self
            .users
            .get_by_email(target_email)
            .await
            .map_err(|e| ShareError::Internal(e.to_string()))?
            .ok_or(ShareError::UserNotFound)?;

        if target.id == account.owner_id {
            return Err(ShareError::Validation(
                "cannot share with the account owner".into(),
            ));
        }

        self.shares
            .upsert(account_id, &target.id, level)
            .await
            .map_err(|e| ShareError::Internal(e.to_string()))?;

        // Audit: account shared.
        let event = pfm_domain::AuditEvent::new(pfm_domain::AuditAction::AccountShared)
            .with_actor(actor_id)
            .with_entity("account", account_id);
        let _ = self.users.log(&event).await;

        Ok(self
            .shares
            .get(account_id, &target.id)
            .await
            .map_err(|e| ShareError::Internal(e.to_string()))?
            .unwrap_or_else(|| ShareEntry {
                id: String::new(),
                account_id: account_id.to_string(),
                user_id: target.id,
                level,
                created_at: String::new(),
            }))
    }

    /// Revoke a share on `account_id` for the given `target_user_id`.
    /// Only the account owner or an admin may call this.
    pub async fn remove_share(
        &self,
        actor_id: &str,
        account_id: &str,
        target_user_id: &str,
    ) -> Result<(), ShareError> {
        if !self.check_owner_or_admin(actor_id, account_id).await? {
            return Err(ShareError::Forbidden);
        }

        self.shares
            .remove(account_id, target_user_id)
            .await
            .map_err(|e| ShareError::Internal(e.to_string()))?;

        // Audit: account unshared.
        let event = pfm_domain::AuditEvent::new(pfm_domain::AuditAction::AccountUnshared)
            .with_actor(actor_id)
            .with_entity("account", account_id);
        let _ = self.users.log(&event).await;

        Ok(())
    }

    /// Binary email search: returns `true` only if the given email belongs to a
    /// user who has a share on this account. The response is deliberately
    /// non-revealing: it never exposes names or other user data.
    ///
    /// Even so, the caller must own the account (or be an admin): without that
    /// check the endpoint is an oracle that confirms whether an arbitrary email
    /// belongs to a registered user.
    pub async fn search_email(
        &self,
        actor_id: &str,
        account_id: &str,
        email: &str,
    ) -> Result<bool, ShareError> {
        if !self.check_owner_or_admin(actor_id, account_id).await? {
            return Err(ShareError::Forbidden);
        }

        self.shares
            .search_email(account_id, email)
            .await
            .map_err(|e| ShareError::Internal(e.to_string()))
    }

    /// List all shares on a given account. Requires owner or admin.
    pub async fn list_shares(
        &self,
        actor_id: &str,
        account_id: &str,
    ) -> Result<Vec<ShareEntry>, ShareError> {
        if !self.check_owner_or_admin(actor_id, account_id).await? {
            return Err(ShareError::Forbidden);
        }

        self.shares
            .list_for_account(account_id)
            .await
            .map_err(|e| ShareError::Internal(e.to_string()))
    }
}
