//! Audit-log domain types.
//!
//! The audit trail is append-only and recorded for authentication events and,
//! from later steps, for other mutations. Temporal fields are carried as
//! RFC-3339 UTC strings so the domain stays `chrono`-free; the repository
//! converts at the boundary.

/// A discrete, auditable action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuditAction {
    /// A successful login.
    Login,
    /// A failed login attempt.
    LoginFailed,
    /// A logout (session destroyed).
    Logout,
    /// A password change.
    PasswordChanged,
    /// An admin created a user.
    UserCreated,
    /// An admin updated a user (role, active, etc.).
    UserUpdated,
    /// An admin deleted a user.
    UserDeleted,
    /// An admin started an impersonation session.
    ImpersonationStarted,
    /// An admin stopped an impersonation session.
    ImpersonationStopped,
    /// An admin reassigned a user's accounts before deletion.
    UserAccountsReassigned,
    /// An admin deleted an account.
    AccountDeleted,
    /// An account owner deleted their own account.
    AccountSelfDeleted,
    /// An account was shared with another user.
    AccountShared,
    /// A share on an account was revoked.
    AccountUnshared,
    /// A CSV import was committed to an account.
    ImportCommitted,
}

impl AuditAction {
    /// Stable string form used on the wire and in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            AuditAction::Login => "login",
            AuditAction::LoginFailed => "login_failed",
            AuditAction::Logout => "logout",
            AuditAction::PasswordChanged => "password_changed",
            AuditAction::UserCreated => "user_created",
            AuditAction::UserUpdated => "user_updated",
            AuditAction::UserDeleted => "user_deleted",
            AuditAction::ImpersonationStarted => "impersonation_started",
            AuditAction::ImpersonationStopped => "impersonation_stopped",
            AuditAction::UserAccountsReassigned => "user_accounts_reassigned",
            AuditAction::AccountDeleted => "account_deleted",
            AuditAction::AccountSelfDeleted => "account_self_deleted",
            AuditAction::AccountShared => "account_shared",
            AuditAction::AccountUnshared => "account_unshared",
            AuditAction::ImportCommitted => "import_committed",
        }
    }

    /// Parse an action from its string form. `None` for unknown values.
    pub fn parse(s: &str) -> Option<AuditAction> {
        Some(match s {
            "login" => AuditAction::Login,
            "login_failed" => AuditAction::LoginFailed,
            "logout" => AuditAction::Logout,
            "password_changed" => AuditAction::PasswordChanged,
            "user_created" => AuditAction::UserCreated,
            "user_updated" => AuditAction::UserUpdated,
            "user_deleted" => AuditAction::UserDeleted,
            "impersonation_started" => AuditAction::ImpersonationStarted,
            "impersonation_stopped" => AuditAction::ImpersonationStopped,
            "user_accounts_reassigned" => AuditAction::UserAccountsReassigned,
            "account_deleted" => AuditAction::AccountDeleted,
            "account_self_deleted" => AuditAction::AccountSelfDeleted,
            "account_shared" => AuditAction::AccountShared,
            "account_unshared" => AuditAction::AccountUnshared,
            "import_committed" => AuditAction::ImportCommitted,
            _ => return None,
        })
    }
}

impl std::fmt::Display for AuditAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The input to record an audit entry. The repository supplies the row
/// identity (`id`) and the event timestamp (`at`) when it persists the row.
#[derive(Debug, Clone)]
pub struct AuditEvent {
    /// The acting user, or `None` when there is no authenticated session yet
    /// (e.g. a failed login).
    pub actor_user_id: Option<String>,
    /// The user being impersonated. Always `None` until impersonation lands in
    /// a later step; the column is kept nullable and written as NULL for now.
    pub impersonated_as_user_id: Option<String>,
    pub action: AuditAction,
    /// Affected entity kind (e.g. `"user"`), if any.
    pub entity: Option<String>,
    /// Affected entity identifier, if any.
    pub entity_id: Option<String>,
    /// Client IP address, if known.
    pub ip: Option<String>,
    /// Client `User-Agent`, if sent.
    pub user_agent: Option<String>,
}

impl AuditEvent {
    pub fn new(action: AuditAction) -> Self {
        Self {
            actor_user_id: None,
            impersonated_as_user_id: None,
            action,
            entity: None,
            entity_id: None,
            ip: None,
            user_agent: None,
        }
    }

    pub fn with_actor(mut self, actor_user_id: impl Into<String>) -> Self {
        self.actor_user_id = Some(actor_user_id.into());
        self
    }

    pub fn with_impersonated_as(mut self, user_id: impl Into<String>) -> Self {
        self.impersonated_as_user_id = Some(user_id.into());
        self
    }

    pub fn with_entity(mut self, entity: impl Into<String>, entity_id: impl Into<String>) -> Self {
        self.entity = Some(entity.into());
        self.entity_id = Some(entity_id.into());
        self
    }

    pub fn with_ip(mut self, ip: impl Into<String>) -> Self {
        self.ip = Some(ip.into());
        self
    }

    pub fn with_user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = Some(user_agent.into());
        self
    }
}

/// A persisted audit row (the full record, as read back from the database).
#[derive(Debug, Clone)]
pub struct AuditRecord {
    pub id: String,
    pub actor_user_id: Option<String>,
    pub impersonated_as_user_id: Option<String>,
    pub action: AuditAction,
    pub entity: Option<String>,
    pub entity_id: Option<String>,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    /// Event timestamp, RFC-3339 UTC.
    pub at: String,
}
