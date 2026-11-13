//! Pure password-related commands. The actual Argon2id hashing / verification
//! lives in `pfm-service` (it needs the `argon2` crate and the cost parameters
//! from configuration); this module only carries the intent.

/// Request to change an authenticated user's password. The current password is
/// required so the client proves possession before the secret is rotated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangePassword {
    pub current_password: String,
    pub new_password: String,
}

impl ChangePassword {
    pub fn new(current_password: impl Into<String>, new_password: impl Into<String>) -> Self {
        Self {
            current_password: current_password.into(),
            new_password: new_password.into(),
        }
    }
}
