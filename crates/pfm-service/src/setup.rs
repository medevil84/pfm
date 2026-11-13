//! Setup service: orchestrates the first-time configuration wizard backend.
//!
//! * [`SetupService::is_configured`] – combined check (marker + admin user).
//! * [`SetupService::reset_all_data`] – atomic wipe of all application data.
//! * [`SetupService::complete`] – validate, hash password, create admin user
//!   + prefs + marker in a single transaction (delegated to the repo).

use pfm_repo::{SetupCompleteData, SetupRepo, SqlxSetupRepo};
use thiserror::Error;

use crate::PasswordService;

/// Error type for setup operations.
#[derive(Debug, Error)]
pub enum SetupError {
    #[error("password is required and must be at least 8 characters")]
    InvalidPassword,
    #[error("username must be 3–64 characters of [a-z0-9_-]")]
    InvalidUsername,
    #[error("email must be a valid address")]
    InvalidEmail,
    #[error("username is already taken")]
    UsernameTaken,
    #[error("email is already taken")]
    EmailTaken,
    #[error("unsupported locale: {0}")]
    UnsupportedLocale(String),
    #[error("invalid currency code: {0}")]
    InvalidCurrency(String),
    #[error("reset failed: {0}")]
    ResetFailed(String),
    #[error("internal error: {0}")]
    Internal(String),
}

/// Parameters for completing the initial setup.
#[derive(Debug, Clone)]
pub struct SetupCompleteParams {
    pub username: String,
    pub password: String,
    pub email: String,
    pub app_name: String,
    pub locale: String,
    pub currency: String,
}

/// Business-logic service for the setup wizard.
pub struct SetupService {
    repo: SqlxSetupRepo,
    passwords: PasswordService,
}

impl std::fmt::Debug for SetupService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SetupService").finish()
    }
}

impl SetupService {
    pub fn new(repo: &SqlxSetupRepo, passwords: PasswordService) -> Self {
        Self {
            repo: repo.clone(),
            passwords,
        }
    }

    /// Returns `true` when the database has completed its initial setup:
    /// the `setup_state` marker is set AND at least one active admin user
    /// exists.
    pub async fn is_configured(&self) -> Result<bool, SetupError> {
        let marker = self
            .repo
            .is_initialized()
            .await
            .map_err(|e| SetupError::Internal(e.to_string()))?;
        if !marker {
            return Ok(false);
        }
        let has_admin = self
            .repo
            .has_admin_user()
            .await
            .map_err(|e| SetupError::Internal(e.to_string()))?;
        Ok(has_admin)
    }

    /// Reset all application data. After success the database is in the same
    /// state as a freshly-migrated installation.
    pub async fn reset_all_data(&self) -> Result<(), SetupError> {
        self.repo
            .reset_all_data()
            .await
            .map_err(|e| SetupError::ResetFailed(e.to_string()))
    }

    /// Complete the initial setup: validate inputs, hash the password, then
    /// atomically create the admin user, their preferences, and the
    /// `setup_state` marker.
    pub async fn complete(&self, params: &SetupCompleteParams) -> Result<(), SetupError> {
        // Validate inputs (backend-side, not trusting the client).
        validate_setup_params(params)?;

        // Hash the password with Argon2id.
        let password_hash = self
            .passwords
            .hash(&params.password)
            .map_err(|e| SetupError::Internal(e.to_string()))?;

        let data = SetupCompleteData {
            user_id: uuid::Uuid::new_v4().to_string(),
            username: params.username.trim().to_string(),
            email: params.email.trim().to_string(),
            password_hash,
            locale: params.locale.clone(),
            default_currency: params.currency.clone(),
        };

        self.repo.complete_setup(&data).await.map_err(|e| {
            let msg = e.to_string();
            if msg.contains("UNIQUE") {
                // Distinguish username vs email conflict.
                if msg.contains("username") {
                    SetupError::UsernameTaken
                } else {
                    SetupError::EmailTaken
                }
            } else {
                SetupError::Internal(msg)
            }
        })
    }
}

/// Validate setup parameters on the backend (not trusting the client).
fn validate_setup_params(params: &SetupCompleteParams) -> Result<(), SetupError> {
    // Username: 3–64 chars of [a-z0-9_-]
    let username = params.username.trim();
    if username.len() < 3 || username.len() > 64 {
        return Err(SetupError::InvalidUsername);
    }
    if !username
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        return Err(SetupError::InvalidUsername);
    }

    // Password: required, minimum 8 characters.
    if params.password.is_empty() || params.password.len() < 8 {
        return Err(SetupError::InvalidPassword);
    }

    // Email: basic format check.
    let email = params.email.trim();
    if !email.contains('@') || !email.contains('.') {
        return Err(SetupError::InvalidEmail);
    }
    let at_pos = email.find('@').unwrap();
    if at_pos == 0 || at_pos + 1 >= email.len() {
        return Err(SetupError::InvalidEmail);
    }
    let domain = &email[at_pos + 1..];
    if !domain.contains('.') || domain.starts_with('.') || domain.ends_with('.') {
        return Err(SetupError::InvalidEmail);
    }

    // Locale: must be 'it' or 'en'.
    if params.locale != "it" && params.locale != "en" {
        return Err(SetupError::UnsupportedLocale(params.locale.clone()));
    }

    // Currency: must be a valid ISO 4217 code (3 uppercase letters).
    if params.currency.len() != 3 || !params.currency.chars().all(|c| c.is_ascii_uppercase()) {
        return Err(SetupError::InvalidCurrency(params.currency.clone()));
    }

    Ok(())
}
