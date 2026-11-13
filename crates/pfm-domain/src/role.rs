//! The RBAC role. Kept as a plain enum (no `serde`) so the domain stays
//! serialization-free; the HTTP layer maps it to/from strings.

/// Application role. Both values are persisted; enforcement on accounts is a
/// later step (step 17).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Full access (user management, impersonation, ...).
    Admin,
    /// Standard end user.
    User,
}

impl Role {
    /// Stable string form used on the wire and in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::User => "user",
        }
    }

    /// Parse a role from its string form. `None` for unknown values.
    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "admin" => Some(Role::Admin),
            "user" => Some(Role::User),
            _ => None,
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
