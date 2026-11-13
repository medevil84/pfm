//! Pure permission-check functions for account sharing.
//!
//! This module is intentionally free of I/O, serialization, or framework
//! dependencies. Callers (the service layer) pre-load the relevant data from
//! the database and pass it here; the functions themselves are simple
//! predicates.

use crate::role::Role;

/// The access level granted to a shared user on an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShareLevel {
    /// Read-only access: the user can view the account and its data.
    Read,
    /// Read-write access: the user can view and mutate the account.
    Write,
}

impl ShareLevel {
    /// Stable string form used on the wire and in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            ShareLevel::Read => "read",
            ShareLevel::Write => "write",
        }
    }

    /// Parse a level from its string form. `None` for unknown values.
    pub fn parse(s: &str) -> Option<ShareLevel> {
        Some(match s {
            "read" => ShareLevel::Read,
            "write" => ShareLevel::Write,
            _ => return None,
        })
    }
}

impl std::fmt::Display for ShareLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A persisted share row: which user has what level on an account.
#[derive(Debug, Clone)]
pub struct ShareEntry {
    /// Opaque identifier (UUID string, application-generated).
    pub id: String,
    /// The account this share applies to.
    pub account_id: String,
    /// The user this share is granted to.
    pub user_id: String,
    /// The access level.
    pub level: ShareLevel,
    /// Creation timestamp, RFC-3339 UTC.
    pub created_at: String,
}

/// Returns `true` if the actor can **see** (read) the account.
///
/// - Admins always can.
/// - The owner always can.
/// - Any share entry (read or write) grants visibility.
pub fn can_see(actor_role: Role, actor_id: &str, owner_id: &str, has_share: bool) -> bool {
    actor_role == Role::Admin || actor_id == owner_id || has_share
}

/// Returns `true` if the actor can **write** (mutate) the account.
///
/// - Admins always can.
/// - The owner always can.
/// - Only a `Write`-level share grants write access.
pub fn can_write(
    actor_role: Role,
    actor_id: &str,
    owner_id: &str,
    share_level: Option<ShareLevel>,
) -> bool {
    actor_role == Role::Admin || actor_id == owner_id || share_level == Some(ShareLevel::Write)
}
