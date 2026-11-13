//! Input validation helpers for the HTTP layer.
//!
//! Every DTO that is written to the database must pass explicit validation
//! before reaching the service layer. These functions enforce:
//!
//! - Maximum string lengths (prevents unbounded memory / DB overflow)
//! - IBAN format (country prefix + 2 check digits + alphanumeric body)
//! - Email format (basic RFC 5322: `local@domain`)
//! - Amount bounds (absolute ceiling on cent values)

use axum::http::StatusCode;

use crate::error::ApiError;

// ─── Constants ───────────────────────────────────────────────────────────────

/// Maximum length for a user-visible name (account, category, rule, etc.).
pub const MAX_NAME_LEN: usize = 128;
/// Maximum length for a free-text note or memo.
pub const MAX_NOTE_LEN: usize = 1024;
/// Maximum length for a transaction description.
pub const MAX_DESCRIPTION_LEN: usize = 512;
/// Maximum length for an IBAN (SEPA max is 34 chars).
pub const MAX_IBAN_LEN: usize = 34;
/// Maximum length for an email address.
pub const MAX_EMAIL_LEN: usize = 254;
/// Maximum length for a username.
pub const MAX_USERNAME_LEN: usize = 64;
/// Maximum length for a password (before hashing).
pub const MAX_PASSWORD_LEN: usize = 128;
/// Minimum password length.
pub const MIN_PASSWORD_LEN: usize = 8;
/// Absolute ceiling on a single amount in cents (±100 billion in cents = ±1 billion in major units).
pub const MAX_CENTS_ABS: i64 = 100_000_000_000;

// ─── Validators ──────────────────────────────────────────────────────────────

/// Require `Content-Type: application/json` on handlers that parse the body by
/// hand. Without it a cross-site `<form enctype="text/plain">` could submit a
/// JSON-looking payload (no CORS pre-flight is triggered for such requests),
/// which matters for the CSRF-exempt login endpoint.
pub fn require_json_content_type(headers: &axum::http::HeaderMap) -> Result<(), ApiError> {
    let is_json = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            let essence = v
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            essence == "application/json"
                || (essence.starts_with("application/") && essence.ends_with("+json"))
        })
        .unwrap_or(false);
    if is_json {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "errors.invalid_json",
        ))
    }
}

/// Validate that a non-empty string does not exceed `max_len` characters.
pub fn check_len(s: &str, max_len: usize, field: &str) -> Result<(), ApiError> {
    if s.is_empty() {
        return Err(ApiError::with_args(
            StatusCode::BAD_REQUEST,
            "errors.validation",
            format!("{field} must not be empty"),
        ));
    }
    if s.len() > max_len {
        return Err(ApiError::with_args(
            StatusCode::BAD_REQUEST,
            "errors.validation",
            format!(
                "{field} must be at most {max_len} characters (got {})",
                s.len()
            ),
        ));
    }
    Ok(())
}

/// Validate an account icon: a bootstrap-icons class name such as `bi-bank`.
/// The value is rendered into a `class="…"` attribute, so anything outside
/// `[A-Za-z0-9_-]` (quotes, angle brackets, spaces) is refused.
pub fn check_icon(icon: &str) -> Result<(), ApiError> {
    let ok = !icon.is_empty()
        && icon.len() <= 40
        && icon
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(ApiError::with_args(
            StatusCode::BAD_REQUEST,
            "errors.validation",
            "icon must be 1-40 characters of [A-Za-z0-9_-]",
        ))
    }
}

/// Validate a category emoji: short, printable, and free of the characters that
/// are significant in HTML (the value is interpolated into markup).
pub fn check_emoji(emoji: &str) -> Result<(), ApiError> {
    let n = emoji.chars().count();
    let ok = (1..=16).contains(&n)
        && !emoji.trim().is_empty()
        && !emoji
            .chars()
            .any(|c| c.is_control() || matches!(c, '<' | '>' | '&' | '"' | '\'' | '`'));
    if ok {
        Ok(())
    } else {
        Err(ApiError::with_args(
            StatusCode::UNPROCESSABLE_ENTITY,
            "errors.validation",
            "emoji must be 1-16 printable characters without < > & \" ' `",
        ))
    }
}

/// Validate an optional string field: if present, must not exceed `max_len`.
/// Empty strings are rejected unless `allow_empty` is true.
pub fn check_len_opt(
    s: &Option<String>,
    max_len: usize,
    field: &str,
    allow_empty: bool,
) -> Result<(), ApiError> {
    if let Some(val) = s {
        if !allow_empty && val.is_empty() {
            return Err(ApiError::with_args(
                StatusCode::BAD_REQUEST,
                "errors.validation",
                format!("{field} must not be empty"),
            ));
        }
        if val.len() > max_len {
            return Err(ApiError::with_args(
                StatusCode::BAD_REQUEST,
                "errors.validation",
                format!(
                    "{field} must be at most {max_len} characters (got {})",
                    val.len()
                ),
            ));
        }
    }
    Ok(())
}

/// Validate that an IBAN string is well-formed:
/// - 15 to 34 characters
/// - starts with a 2-letter country code (uppercase A-Z)
/// - followed by 2 check digits (0-9)
/// - remaining characters are alphanumeric (A-Z0-9)
pub fn check_iban(iban: &str) -> Result<(), ApiError> {
    let len = iban.len();
    if !(15..=MAX_IBAN_LEN).contains(&len) {
        return Err(ApiError::with_args(
            StatusCode::BAD_REQUEST,
            "errors.validation",
            format!("iban must be 15-{MAX_IBAN_LEN} characters (got {len})"),
        ));
    }
    let bytes = iban.as_bytes();
    // Country code: 2 uppercase letters
    if !bytes[0].is_ascii_uppercase() || !bytes[1].is_ascii_uppercase() {
        return Err(ApiError::with_args(
            StatusCode::BAD_REQUEST,
            "errors.validation",
            "iban must start with a 2-letter country code",
        ));
    }
    // Check digits: 2 digits
    if !bytes[2].is_ascii_digit() || !bytes[3].is_ascii_digit() {
        return Err(ApiError::with_args(
            StatusCode::BAD_REQUEST,
            "errors.validation",
            "iban characters 3-4 must be digits (check digits)",
        ));
    }
    // BBAN: alphanumeric
    if !iban[4..].chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(ApiError::with_args(
            StatusCode::BAD_REQUEST,
            "errors.validation",
            "iban body must be alphanumeric (A-Z, 0-9)",
        ));
    }
    Ok(())
}

/// Validate an optional IBAN: if `Some`, must pass `check_iban`.
pub fn check_iban_opt(iban: &Option<String>) -> Result<(), ApiError> {
    if let Some(ref v) = iban {
        check_iban(v)?;
    }
    Ok(())
}

/// Validate a basic email format: `local@domain` where domain has at least
/// one dot. Does not perform DNS lookup or MX validation.
pub fn check_email(email: &str) -> Result<(), ApiError> {
    if email.len() > MAX_EMAIL_LEN {
        return Err(ApiError::with_args(
            StatusCode::BAD_REQUEST,
            "errors.validation",
            format!("email must be at most {MAX_EMAIL_LEN} characters"),
        ));
    }
    let at_pos = email.find('@').ok_or_else(|| {
        ApiError::with_args(
            StatusCode::BAD_REQUEST,
            "errors.validation",
            "email must contain '@'",
        )
    })?;
    if at_pos == 0 || at_pos + 1 >= email.len() {
        return Err(ApiError::with_args(
            StatusCode::BAD_REQUEST,
            "errors.validation",
            "email local-part and domain must be non-empty",
        ));
    }
    let domain = &email[at_pos + 1..];
    if !domain.contains('.') {
        return Err(ApiError::with_args(
            StatusCode::BAD_REQUEST,
            "errors.validation",
            "email domain must contain a dot",
        ));
    }
    // Reject spaces and control characters
    if email.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ApiError::with_args(
            StatusCode::BAD_REQUEST,
            "errors.validation",
            "email must not contain whitespace or control characters",
        ));
    }
    Ok(())
}

/// Validate a password: length within bounds.
pub fn check_password(pw: &str) -> Result<(), ApiError> {
    if pw.len() < MIN_PASSWORD_LEN {
        return Err(ApiError::with_args(
            StatusCode::BAD_REQUEST,
            "errors.validation",
            format!("password must be at least {MIN_PASSWORD_LEN} characters"),
        ));
    }
    if pw.len() > MAX_PASSWORD_LEN {
        return Err(ApiError::with_args(
            StatusCode::BAD_REQUEST,
            "errors.validation",
            format!("password must be at most {MAX_PASSWORD_LEN} characters"),
        ));
    }
    Ok(())
}

/// Validate that a cent value is within the absolute bounds.
pub fn check_cents(cents: i64) -> Result<(), ApiError> {
    if cents.abs() > MAX_CENTS_ABS {
        return Err(ApiError::with_args(
            StatusCode::BAD_REQUEST,
            "errors.validation",
            format!(
                "amount must be within ±{} cents (got {cents})",
                MAX_CENTS_ABS
            ),
        ));
    }
    Ok(())
}

/// Validate an optional cent value.
pub fn check_cents_opt(cents: &Option<i64>) -> Result<(), ApiError> {
    if let Some(c) = cents {
        check_cents(*c)?;
    }
    Ok(())
}

// ─── Convenience: validate a full set of fields ─────────────────────────────

/// Validate common account fields (name, note, iban).
pub fn validate_account_fields(
    name: &str,
    note: &Option<String>,
    iban: &Option<String>,
) -> Result<(), ApiError> {
    check_len(name, MAX_NAME_LEN, "name")?;
    check_len_opt(note, MAX_NOTE_LEN, "note", true)?;
    check_iban_opt(iban)?;
    Ok(())
}

/// Validate common transaction fields (description, note, cents).
pub fn validate_tx_fields(
    description: &Option<String>,
    note: &Option<String>,
    cents: i64,
) -> Result<(), ApiError> {
    check_len_opt(description, MAX_DESCRIPTION_LEN, "description", true)?;
    check_len_opt(note, MAX_NOTE_LEN, "note", true)?;
    check_cents(cents)?;
    Ok(())
}
