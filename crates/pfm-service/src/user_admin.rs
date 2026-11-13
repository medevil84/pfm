//! User administration use-cases: CRUD for users, account reassignment on
//! delete, and audit logging with impersonation awareness.

use pfm_domain::{AuditAction, AuditEvent, Role, User};
use pfm_repo::{AccountsRepo, AuditRepo, SqlxAccountsRepo, SqlxAuthRepo, Storage, UsersRepo};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum UserAdminError {
    #[error("user not found")]
    UserNotFound,
    #[error("account not found")]
    AccountNotFound,
    #[error("username already exists")]
    UsernameTaken,
    #[error("email already exists")]
    EmailTaken,
    #[error("user has accounts that need reassignment: {0}")]
    AccountsNeedReassignment(String),
    #[error("account has transactions that need reassignment")]
    AccountHasTransactions,
    #[error("validation error: {0}")]
    Validation(String),
    #[error("cannot delete the last admin")]
    LastAdmin,
    #[error("internal error: {0}")]
    Internal(String),
}

/// Input for creating a new user.
#[derive(Debug)]
pub struct UserCreateCmd {
    pub username: String,
    pub email: String,
    pub password: String,
    pub role: Role,
    pub locale: String,
    pub default_currency: String,
}

/// Input for updating an existing user. All fields `Option`; `None` = keep.
#[derive(Debug, Default)]
pub struct UserUpdateCmd {
    pub role: Option<Role>,
    pub active: Option<bool>,
    pub locale: Option<String>,
    pub default_currency: Option<String>,
    /// New plaintext password (admin reset); `None` = keep the current one.
    pub password: Option<String>,
}

/// The reassignment strategy for a user's accounts on deletion.
#[derive(Debug)]
pub enum AccountReassignment {
    /// Reassign all accounts to the specified target user.
    ReassignTo(String),
    /// Block deletion: accounts exist and no target was given.
    Block,
}

/// The reassignment strategy for an account's transactions on deletion.
#[derive(Debug)]
pub enum TransactionReassignment {
    /// Reassign all transactions to the specified target account.
    ReassignTo(String),
    /// Block deletion: transactions exist and no target was given.
    Block,
}

/// A lightweight admin view of an account paired with its owner's username.
#[derive(Debug)]
pub struct AdminAccountInfo {
    pub id: String,
    pub owner_id: String,
    pub owner_username: String,
    pub name: String,
    pub kind: String,
    pub currency: String,
    pub active: bool,
}

/// User administration service: create, read, update, and delete users with
/// explicit reassignment of accounts and transactions. All mutations write to
/// the audit log with the real admin as actor.
pub struct UserAdminService {
    users: SqlxAuthRepo,
    accounts: SqlxAccountsRepo,
    categories: pfm_repo::SqlxCategoriesRepo,
    tags: pfm_repo::SqlxTagsRepo,
    passwords: PasswordServiceRef,
}

/// A lightweight reference to the password hashing service.
/// We store the parameters and construct Argon2 on the fly to avoid
/// a direct dependency on `PasswordService` (which lives in `pfm-service`
/// root and is already available to the caller).
pub struct PasswordServiceRef {
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
}

impl PasswordServiceRef {
    pub fn new(memory_kib: u32, iterations: u32, parallelism: u32) -> Self {
        Self {
            memory_kib,
            iterations,
            parallelism,
        }
    }

    pub fn hash(&self, password: &str) -> Result<String, UserAdminError> {
        use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
        use argon2::{Algorithm, Argon2, Params, Version};

        let salt = SaltString::generate(&mut OsRng);
        let params = Params::new(self.memory_kib, self.iterations, self.parallelism, None)
            .map_err(|e| UserAdminError::Internal(e.to_string()))?;
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let hash = argon2
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| UserAdminError::Internal(e.to_string()))?;
        Ok(hash.to_string())
    }
}

impl UserAdminService {
    pub fn new(storage: &Storage, passwords: PasswordServiceRef) -> Self {
        Self {
            users: SqlxAuthRepo::new(storage),
            accounts: SqlxAccountsRepo::new(storage),
            categories: pfm_repo::SqlxCategoriesRepo::new(storage),
            tags: pfm_repo::SqlxTagsRepo::new(storage),
            passwords,
        }
    }

    /// List all users (admin-only).
    pub async fn list_users(&self) -> Result<Vec<User>, UserAdminError> {
        self.users
            .list_all()
            .await
            .map_err(|e| UserAdminError::Internal(e.to_string()))
    }

    /// List all accounts in the system with owner info (admin-only).
    pub async fn list_all_accounts(
        &self,
    ) -> Result<Vec<crate::user_admin::AdminAccountInfo>, UserAdminError> {
        let accounts = self
            .accounts
            .list_all()
            .await
            .map_err(|e| UserAdminError::Internal(e.to_string()))?;

        let mut result = Vec::with_capacity(accounts.len());
        for account in &accounts {
            let owner = self
                .users
                .get_by_id(&account.owner_id)
                .await
                .map_err(|e| UserAdminError::Internal(e.to_string()))?;
            let owner_username = owner
                .map(|u| u.username)
                .unwrap_or_else(|| String::from("unknown"));

            result.push(AdminAccountInfo {
                id: account.id.clone(),
                owner_id: account.owner_id.clone(),
                owner_username,
                name: account.name.clone(),
                kind: account.kind.as_str().to_string(),
                currency: account.currency.clone(),
                active: account.active,
            });
        }
        Ok(result)
    }

    /// Get a single user by id.
    pub async fn get_user(&self, user_id: &str) -> Result<User, UserAdminError> {
        self.users
            .get_by_id(user_id)
            .await
            .map_err(|e| UserAdminError::Internal(e.to_string()))?
            .ok_or(UserAdminError::UserNotFound)
    }

    /// Create a new user.
    pub async fn create_user(
        &self,
        actor_id: &str,
        cmd: &UserCreateCmd,
        impersonated_as: Option<&str>,
    ) -> Result<User, UserAdminError> {
        // Validate uniqueness
        if self
            .users
            .get_by_username(&cmd.username)
            .await
            .ok()
            .flatten()
            .is_some()
        {
            return Err(UserAdminError::UsernameTaken);
        }
        if self
            .users
            .get_by_email(&cmd.email)
            .await
            .ok()
            .flatten()
            .is_some()
        {
            return Err(UserAdminError::EmailTaken);
        }

        let password_hash = self.passwords.hash(&cmd.password)?;

        let user = self
            .users
            .create(
                &cmd.username,
                &cmd.email,
                &password_hash,
                cmd.role.as_str(),
                &cmd.locale,
                &cmd.default_currency,
            )
            .await
            .map_err(|e| UserAdminError::Internal(e.to_string()))?;

        // Audit
        let mut event = AuditEvent::new(AuditAction::UserCreated)
            .with_actor(actor_id)
            .with_entity("user", &user.id);
        if let Some(imp) = impersonated_as {
            event = event.with_impersonated_as(imp);
        }
        let _ = self.users.log(&event).await;

        Ok(user)
    }

    /// Update a user's mutable fields.
    pub async fn update_user(
        &self,
        actor_id: &str,
        user_id: &str,
        cmd: &UserUpdateCmd,
        impersonated_as: Option<&str>,
    ) -> Result<User, UserAdminError> {
        let user = self
            .users
            .get_by_id(user_id)
            .await
            .map_err(|e| UserAdminError::Internal(e.to_string()))?
            .ok_or(UserAdminError::UserNotFound)?;

        let new_role = cmd.role.unwrap_or(user.role);
        let new_active = cmd.active.unwrap_or(user.active);
        let new_locale = cmd.locale.clone().unwrap_or_else(|| user.locale.clone());
        let new_currency = cmd
            .default_currency
            .clone()
            .unwrap_or_else(|| user.default_currency.clone());

        // Guard: cannot demote/deactivate the last admin
        if user.role == Role::Admin && (new_role != Role::Admin || !new_active) {
            let all_users = self
                .users
                .list_all()
                .await
                .map_err(|e| UserAdminError::Internal(e.to_string()))?;
            let other_admins = all_users
                .iter()
                .filter(|u| u.role == Role::Admin && u.active && u.id != user_id)
                .count();
            if other_admins == 0 {
                return Err(UserAdminError::LastAdmin);
            }
        }

        self.users
            .update_user(
                user_id,
                new_role.as_str(),
                new_active,
                &new_locale,
                &new_currency,
            )
            .await
            .map_err(|e| UserAdminError::Internal(e.to_string()))?;

        if let Some(pw) = cmd.password.as_deref() {
            let hash = self.passwords.hash(pw)?;
            self.users
                .update_password_hash(user_id, &hash)
                .await
                .map_err(|e| UserAdminError::Internal(e.to_string()))?;
        }

        let updated = User {
            role: new_role,
            active: new_active,
            locale: new_locale,
            default_currency: new_currency,
            ..user
        };

        // Audit
        let mut event = AuditEvent::new(AuditAction::UserUpdated)
            .with_actor(actor_id)
            .with_entity("user", user_id);
        if let Some(imp) = impersonated_as {
            event = event.with_impersonated_as(imp);
        }
        let _ = self.users.log(&event).await;

        Ok(updated)
    }

    /// Delete a user. All owned accounts must be reassigned to another user
    /// or the operation is blocked.
    pub async fn delete_user(
        &self,
        actor_id: &str,
        user_id: &str,
        reassignment: &AccountReassignment,
        impersonated_as: Option<&str>,
    ) -> Result<(), UserAdminError> {
        let user = self
            .users
            .get_by_id(user_id)
            .await
            .map_err(|e| UserAdminError::Internal(e.to_string()))?
            .ok_or(UserAdminError::UserNotFound)?;

        // Guard: cannot delete the last admin
        if user.role == Role::Admin && user.active {
            let all_users = self
                .users
                .list_all()
                .await
                .map_err(|e| UserAdminError::Internal(e.to_string()))?;
            let other_admins = all_users
                .iter()
                .filter(|u| u.role == Role::Admin && u.active && u.id != user_id)
                .count();
            if other_admins == 0 {
                return Err(UserAdminError::LastAdmin);
            }
        }

        // Find all accounts owned by this user
        let owned_accounts = self
            .accounts
            .list_by_owner(user_id)
            .await
            .map_err(|e| UserAdminError::Internal(e.to_string()))?;

        if !owned_accounts.is_empty() {
            match reassignment {
                AccountReassignment::ReassignTo(target_uid) => {
                    // Verify target exists
                    self.users
                        .get_by_id(target_uid)
                        .await
                        .map_err(|e| UserAdminError::Internal(e.to_string()))?
                        .ok_or(UserAdminError::UserNotFound)?;

                    // Reassign each account's owner_id
                    for account in &owned_accounts {
                        let reassigned = pfm_domain::Account {
                            owner_id: target_uid.clone(),
                            ..account.clone()
                        };
                        self.accounts
                            .update(&reassigned)
                            .await
                            .map_err(|e| UserAdminError::Internal(e.to_string()))?;
                    }

                    // Audit reassignment
                    let mut event = AuditEvent::new(AuditAction::UserAccountsReassigned)
                        .with_actor(actor_id)
                        .with_entity("user", user_id);
                    if let Some(imp) = impersonated_as {
                        event = event.with_impersonated_as(imp);
                    }
                    let _ = self.users.log(&event).await;
                }
                AccountReassignment::Block => {
                    let names: Vec<&str> = owned_accounts.iter().map(|a| a.name.as_str()).collect();
                    return Err(UserAdminError::AccountsNeedReassignment(names.join(", ")));
                }
            }
        }

        // Their categories and tags stay referenced by transactions that
        // other people can still see: turn them into shared rows (managed by
        // an administrator) instead of leaving them with a dangling owner.
        {
            use pfm_repo::{CategoriesRepo, TagsRepo};
            self.categories
                .disown(user_id)
                .await
                .map_err(|e| UserAdminError::Internal(e.to_string()))?;
            self.tags
                .disown(user_id)
                .await
                .map_err(|e| UserAdminError::Internal(e.to_string()))?;
        }

        // Delete the user
        self.users
            .delete_user(user_id)
            .await
            .map_err(|e| UserAdminError::Internal(e.to_string()))?;

        // Audit deletion
        let mut event = AuditEvent::new(AuditAction::UserDeleted)
            .with_actor(actor_id)
            .with_entity("user", user_id);
        if let Some(imp) = impersonated_as {
            event = event.with_impersonated_as(imp);
        }
        let _ = self.users.log(&event).await;

        Ok(())
    }

    /// Delete an account with explicit transaction reassignment.
    pub async fn delete_account(
        &self,
        actor_id: &str,
        account_id: &str,
        tx_reassignment: &TransactionReassignment,
        impersonated_as: Option<&str>,
    ) -> Result<u64, UserAdminError> {
        let _account = self
            .accounts
            .get_by_id(account_id)
            .await
            .map_err(|e| UserAdminError::Internal(e.to_string()))?
            .ok_or(UserAdminError::AccountNotFound)?;

        // Check if referenced as primary
        let referenced = self
            .accounts
            .referenced_as_primary(account_id)
            .await
            .map_err(|e| UserAdminError::Internal(e.to_string()))?;
        if referenced {
            return Err(UserAdminError::Validation(
                "account is referenced as primary by another account".into(),
            ));
        }

        // Check for transactions
        let has_tx = self
            .accounts
            .has_transactions(account_id)
            .await
            .map_err(|e| UserAdminError::Internal(e.to_string()))?;

        if has_tx {
            match tx_reassignment {
                TransactionReassignment::ReassignTo(target_account_id) => {
                    // Verify target account exists
                    self.accounts
                        .get_by_id(target_account_id)
                        .await
                        .map_err(|e| UserAdminError::Internal(e.to_string()))?
                        .ok_or(UserAdminError::AccountNotFound)?;

                    let moved = self
                        .accounts
                        .reassign_transactions(account_id, target_account_id)
                        .await
                        .map_err(|e| UserAdminError::Internal(e.to_string()))?;

                    // Audit
                    let mut event = AuditEvent::new(AuditAction::AccountDeleted)
                        .with_actor(actor_id)
                        .with_entity("account", account_id);
                    if let Some(imp) = impersonated_as {
                        event = event.with_impersonated_as(imp);
                    }
                    let _ = self.users.log(&event).await;

                    self.accounts
                        .delete(account_id)
                        .await
                        .map_err(|e| UserAdminError::Internal(e.to_string()))?;

                    Ok(moved)
                }
                TransactionReassignment::Block => Err(UserAdminError::AccountHasTransactions),
            }
        } else {
            // No transactions, safe to delete
            let mut event = AuditEvent::new(AuditAction::AccountDeleted)
                .with_actor(actor_id)
                .with_entity("account", account_id);
            if let Some(imp) = impersonated_as {
                event = event.with_impersonated_as(imp);
            }
            let _ = self.users.log(&event).await;

            self.accounts
                .delete(account_id)
                .await
                .map_err(|e| UserAdminError::Internal(e.to_string()))?;

            Ok(0)
        }
    }
}
