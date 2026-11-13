//! Login credentials. Plain data carried from the transport layer into the
//! service layer. The password is consumed (hashed / verified) and never logged
//! or echoed back.

/// A username + password pair submitted at login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

impl Credentials {
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
        }
    }
}
