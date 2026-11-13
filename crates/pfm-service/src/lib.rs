//! pfm-service
//!
//! The business-logic (use-case) layer. Services orchestrate domain operations
//! and talk to the data layer exclusively through repository implementors,
//! keeping domain rules independent of transport concerns.

pub mod accounts;
pub mod bulk;
pub mod card;
pub mod categories;
pub mod deposit;
pub mod export;
pub mod fee_rules;
pub mod holidays;
pub mod impersonation;
pub mod import;
pub mod ownership;
pub mod recurrences;
pub mod setup;
pub mod sharing;
pub mod stats;
pub mod tags;
pub mod transactions;
pub mod transfers;
pub mod user_admin;
pub mod virtual_accounts;

pub use pfm_repo::{AuditRepo, PrefsRepo, UsersRepo};

pub use accounts::{AccountError, AccountService};
pub use bulk::{BulkAction, BulkError, BulkService};
pub use card::{CardCatchUpOutcome, CardChargeForecast, CardCycleDays, CardError, CardService};
pub use categories::{CategoryError, CategoryItem, CategoryService};
pub use deposit::{DepositError, DepositInterestOutcome, DepositService};
pub use export::{ExportError, ExportOutput, ExportService};
pub use fee_rules::{
    FeeRuleError, FeeRuleService, RuleCreateCmd, RulePreview, RuleRangeCmd, RuleUpdateCmd,
};
pub use holidays::{HolidayError, HolidayService};
pub use impersonation::{ImpersonationError, ImpersonationService};
pub use import::{
    CommitResult, ImportError, ImportParams, ImportService, ImportedRow, PreviewResult,
};
pub use ownership::{CategoryAccess, CategoryAccessError};
pub use recurrences::{
    AlignReport, DateImpact, OverrideCmd, RecurrenceCreateCmd, RecurrenceError, RecurrenceService,
    RecurrenceUpdateCmd, StaleAction,
};
pub use setup::{SetupCompleteParams, SetupError, SetupService};
pub use sharing::{ShareError, SharingService};
pub use stats::{BalancePoint, CategorySpend, KpiSummary, StatsError, StatsResult, StatsService};
pub use tags::{TagCmd, TagError, TagItem, TagService};
pub use transactions::{
    TransactionCreateCmd, TransactionError, TransactionService, TransactionUpdateCmd,
};
pub use transfers::{TransferCreateCmd, TransferError, TransferService, TransferUpdateCmd};
pub use user_admin::{
    AccountReassignment, AdminAccountInfo, PasswordServiceRef, TransactionReassignment,
    UserAdminError, UserAdminService, UserCreateCmd, UserUpdateCmd,
};
pub use virtual_accounts::{VirtualAccountService, VirtualBalanceView, VirtualError};

use pfm_domain::{AuditAction, AuditEvent, ChangePassword, Credentials, User, UserPrefs};
use pfm_repo::{SqlxAuthRepo, Storage};
use thiserror::Error;

/// Error type for authentication and preference operations.
#[derive(Debug, Error)]
pub enum AuthError {
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("account is not active")]
    AccountInactive,
    #[error("too many password checks in progress")]
    Busy,
    #[error("user not found")]
    UserNotFound,
    #[error("current password is incorrect")]
    IncorrectPassword,
    #[error("internal error: {0}")]
    Internal(String),
}

/// How many Argon2 operations may run at once. Each one needs
/// `memory_kib` of RAM and a full CPU core for tens of milliseconds, so an
/// unbounded number of parallel logins is a memory-exhaustion vector on a
/// small host; excess requests queue (briefly) behind this limit.
const MAX_CONCURRENT_HASHES: usize = 2;

/// How long a request waits for a hashing slot before giving up.
const HASH_QUEUE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Password hashing / verification service using Argon2id.
#[derive(Clone)]
pub struct PasswordService {
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
    slots: std::sync::Arc<tokio::sync::Semaphore>,
    dummy_hash: std::sync::Arc<std::sync::OnceLock<String>>,
}

impl PasswordService {
    pub fn new(memory_kib: u32, iterations: u32, parallelism: u32) -> Self {
        Self {
            memory_kib,
            iterations,
            parallelism,
            slots: std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_HASHES)),
            dummy_hash: std::sync::Arc::new(std::sync::OnceLock::new()),
        }
    }

    /// Verify off the async worker threads and under the concurrency cap.
    pub async fn verify_async(&self, password: &str, hash: &str) -> Result<bool, AuthError> {
        let permit = tokio::time::timeout(HASH_QUEUE_TIMEOUT, self.slots.clone().acquire_owned())
            .await
            .map_err(|_| AuthError::Busy)?
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        let this = self.clone();
        let (password, hash) = (password.to_owned(), hash.to_owned());
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            this.verify(&password, &hash)
        })
        .await
        .map_err(|e| AuthError::Internal(e.to_string()))?
    }

    /// Hash off the async worker threads and under the concurrency cap.
    pub async fn hash_async(&self, password: &str) -> Result<String, AuthError> {
        let permit = tokio::time::timeout(HASH_QUEUE_TIMEOUT, self.slots.clone().acquire_owned())
            .await
            .map_err(|_| AuthError::Busy)?
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        let this = self.clone();
        let password = password.to_owned();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            this.hash(&password)
        })
        .await
        .map_err(|e| AuthError::Internal(e.to_string()))?
    }

    /// A well-formed hash (made with the configured cost) of a random secret,
    /// verified against when the username does not exist so that unknown and
    /// known accounts take the same time to reject.
    async fn dummy_hash(&self) -> Result<String, AuthError> {
        if let Some(h) = self.dummy_hash.get() {
            return Ok(h.clone());
        }
        let secret: String = (0..4).map(|_| uuid::Uuid::new_v4().to_string()).collect();
        // Through the same bounded, off-thread path as every other hash, so a
        // burst of unknown-username logins right after a restart cannot start
        // one 64 MiB hash per worker thread.
        let hash = self.hash_async(&secret).await?;
        Ok(self.dummy_hash.get_or_init(|| hash).clone())
    }

    pub fn hash(&self, password: &str) -> Result<String, AuthError> {
        use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
        use argon2::{Algorithm, Argon2, Params, Version};

        let salt = SaltString::generate(&mut OsRng);
        let params = Params::new(self.memory_kib, self.iterations, self.parallelism, None)
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let hash = argon2
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        Ok(hash.to_string())
    }

    pub fn verify(&self, password: &str, hash: &str) -> Result<bool, AuthError> {
        use argon2::password_hash::{PasswordHash, PasswordVerifier};
        use argon2::Argon2;

        let parsed = PasswordHash::new(hash)
            .map_err(|e| AuthError::Internal(format!("invalid hash format: {e}")))?;
        let argon2 = Argon2::default();
        Ok(argon2.verify_password(password.as_bytes(), &parsed).is_ok())
    }
}

/// Authentication use-cases: login, logout, password change.
pub struct AuthService {
    repo: SqlxAuthRepo,
    passwords: PasswordService,
}

impl AuthService {
    pub fn new(storage: &Storage, passwords: PasswordService) -> Self {
        Self {
            repo: SqlxAuthRepo::new(storage),
            passwords,
        }
    }

    /// Authenticate a user with the given credentials.
    /// On success returns the [`User`]'s auth view; on failure returns
    /// [`AuthError::InvalidCredentials`] or [`AuthError::AccountInactive`].
    pub async fn login(
        &self,
        creds: &Credentials,
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<User, AuthError> {
        let user = self
            .repo
            .get_by_username(&creds.username)
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?;

        let Some(user) = user else {
            // Unknown username: still pay for one Argon2 verification so the
            // response time does not reveal which usernames exist.
            let dummy = self.passwords.dummy_hash().await?;
            let _ = self.passwords.verify_async(&creds.password, &dummy).await?;
            self.audit_failed_login(&creds.username, ip, user_agent)
                .await;
            return Err(AuthError::InvalidCredentials);
        };

        // The password is checked BEFORE the account state: reporting
        // "disabled" to someone who does not know the password would confirm
        // that the account exists.
        let verified = self
            .passwords
            .verify_async(&creds.password, &user.password_hash)
            .await?;

        if !verified {
            self.audit_failed_login(&creds.username, ip, user_agent)
                .await;
            return Err(AuthError::InvalidCredentials);
        }

        if !user.active {
            self.audit_failed_login(&creds.username, ip, user_agent)
                .await;
            return Err(AuthError::AccountInactive);
        }

        let _ = self.repo.touch_last_seen(&user.id).await;
        self.audit_success(&user, ip, user_agent).await;
        Ok(user)
    }

    /// Record a logout audit entry.
    pub async fn logout(
        &self,
        user_id: &str,
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<(), AuthError> {
        let event = AuditEvent::new(AuditAction::Logout)
            .with_actor(user_id)
            .with_entity("user", user_id)
            .with_ip(ip.unwrap_or_default())
            .with_user_agent(user_agent.unwrap_or_default());
        self.repo
            .log(&event)
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        Ok(())
    }

    /// Verify the current password and replace the stored hash.
    pub async fn change_password(
        &self,
        user_id: &str,
        cmd: &ChangePassword,
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<(), AuthError> {
        let user = self
            .repo
            .get_by_id(user_id)
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?
            .ok_or(AuthError::UserNotFound)?;

        let verified = self
            .passwords
            .verify_async(&cmd.current_password, &user.password_hash)
            .await?;
        if !verified {
            return Err(AuthError::IncorrectPassword);
        }

        let new_hash = self.passwords.hash_async(&cmd.new_password).await?;
        self.repo
            .update_password_hash(user_id, &new_hash)
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?;

        let event = AuditEvent::new(AuditAction::PasswordChanged)
            .with_actor(user_id)
            .with_entity("user", user_id)
            .with_ip(ip.unwrap_or_default())
            .with_user_agent(user_agent.unwrap_or_default());
        self.repo
            .log(&event)
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        Ok(())
    }

    /// Persist the user's own locale tag.
    pub async fn set_locale(&self, user_id: &str, locale: &str) -> Result<(), AuthError> {
        self.repo
            .set_locale(user_id, locale)
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))
    }

    /// Look up a user by their id. Returns `None` if not found.
    pub async fn get_user_by_id(&self, user_id: &str) -> Result<Option<User>, AuthError> {
        self.repo
            .get_by_id(user_id)
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))
    }

    async fn audit_success(&self, user: &User, ip: Option<String>, user_agent: Option<String>) {
        let event = AuditEvent::new(AuditAction::Login)
            .with_actor(&user.id)
            .with_entity("user", &user.id)
            .with_ip(ip.unwrap_or_default())
            .with_user_agent(user_agent.unwrap_or_default());
        let _ = self.repo.log(&event).await;
    }

    async fn audit_failed_login(
        &self,
        username: &str,
        ip: Option<String>,
        user_agent: Option<String>,
    ) {
        let event = AuditEvent::new(AuditAction::LoginFailed)
            .with_entity("user", username)
            .with_ip(ip.unwrap_or_default())
            .with_user_agent(user_agent.unwrap_or_default());
        let _ = self.repo.log(&event).await;
    }
}

/// Per-user display preference operations.
pub struct PrefsService {
    repo: SqlxAuthRepo,
}

impl PrefsService {
    pub fn new(storage: &Storage) -> Self {
        Self {
            repo: SqlxAuthRepo::new(storage),
        }
    }

    pub async fn get(&self, user_id: &str) -> Result<UserPrefs, AuthError> {
        self.repo
            .get(user_id)
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))
    }

    pub async fn update(&self, user_id: &str, prefs: &UserPrefs) -> Result<(), AuthError> {
        self.repo
            .upsert(user_id, prefs)
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))
    }
}
