//! The user entity and its lightweight authenticated view.

use crate::role::Role;

/// A persisted user row (full entity, as stored in the `users` table).
///
/// Temporal columns are carried as ISO-8601 strings so this type stays free of
/// a `chrono` dependency; the repository layer converts them to/from
/// `DateTime<Utc>` at the boundary (the sqlx `Any` driver cannot decode chrono
/// types, so the boundary keeps everything in shared string form).
#[derive(Debug, Clone)]
pub struct User {
    /// Opaque identifier (UUID string, application-generated).
    pub id: String,
    /// Unique login name.
    pub username: String,
    /// Unique e-mail address.
    pub email: String,
    /// Argon2id hash of the password (PHC string). Never plaintext.
    pub password_hash: String,
    pub role: Role,
    /// Whether the account can sign in.
    pub active: bool,
    /// UI locale tag, e.g. `"it"`.
    pub locale: String,
    /// Default ISO 4217 currency code, e.g. `"EUR"`.
    pub default_currency: String,
    /// Last successful activity, RFC-3339 UTC, or `None` if never seen.
    pub last_seen_at: Option<String>,
    /// Account creation timestamp, RFC-3339 UTC.
    pub created_at: String,
}

/// The lightweight, authenticated view of a user.
///
/// This is what is stored in the session and handed to handlers. It deliberately
/// excludes the password hash so it can be serialized into the (encrypted)
/// session cookie and into `/auth/me` responses without leaking secrets.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: String,
    pub username: String,
    pub email: String,
    pub role: Role,
    pub locale: String,
    pub default_currency: String,
    pub active: bool,
}

impl User {
    /// Project the full entity down to its authenticated view.
    pub fn auth(&self) -> AuthUser {
        AuthUser {
            id: self.id.clone(),
            username: self.username.clone(),
            email: self.email.clone(),
            role: self.role,
            locale: self.locale.clone(),
            default_currency: self.default_currency.clone(),
            active: self.active,
        }
    }
}

impl From<AuthUser> for User {
    /// Rebuild a full entity from an authenticated view (e.g. for `last_seen_at`
    /// bookkeeping). The `password_hash` and `created_at` are left empty because
    /// they are not part of the authenticated view and are not needed for the
    /// read paths served from a session.
    fn from(auth: AuthUser) -> Self {
        Self {
            id: auth.id,
            username: auth.username,
            email: auth.email,
            password_hash: String::new(),
            role: auth.role,
            active: auth.active,
            locale: auth.locale,
            default_currency: auth.default_currency,
            last_seen_at: None,
            created_at: String::new(),
        }
    }
}
