//! CSRF double-submit validation as an axum middleware.
//!
//! Applied *inside* the `SessionManagerLayer` so the session is already in
//! request extensions when this runs.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::Method;
use axum::middleware::Next;
use axum::response::Response;
use tower_sessions::Session;

use crate::error::ApiError;
use crate::session::SESSION_CSRF_TOKEN;
use crate::state::AppState;

/// Logical (prefix-insensitive) paths exempt from CSRF validation — no
/// session exists yet, so there is no stored token to check against.
///
/// The `/api` mount prefix is stripped before matching (see below), so this
/// list works both in production (where API routes are nested under `/api`)
/// and in the integration tests (which hit the base paths directly).
const CSRF_EXEMPT: &[&str] = &[
    "/auth/login",
    "/setup/reset",
    "/setup/complete",
    "/setup/cancel",
];

/// Validate the `X-CSRF-Token` header against the session-stored token on
/// mutating requests. Read-only methods and exempt paths pass through.
pub async fn csrf_middleware(
    State(_state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    // Every method that is not explicitly read-only is validated (an
    // allow-list of "safe" verbs, not of dangerous ones): a PATCH route added
    // later, or a verb we did not think of, must not silently skip the check.
    let method = req.method().clone();
    if matches!(method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return Ok(next.run(req).await);
    }

    // Strip the optional `/api/` mount prefix so the exemption matches both
    // production paths (`/api/auth/login`) and test paths (`/auth/login`).
    let raw = req.uri().path();
    let logical = match raw.strip_prefix("/api/") {
        Some(rem) => format!("/{}", rem),
        None => raw.to_string(),
    };
    if CSRF_EXEMPT.contains(&logical.as_str()) {
        return Ok(next.run(req).await);
    }

    // The session must be present (set by SessionManagerLayer outside us).
    let session = req
        .extensions()
        .get::<Session>()
        .cloned()
        .ok_or_else(ApiError::unauthorized)?;

    // Token must be in the header.
    let header_token = req
        .headers()
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .ok_or_else(|| ApiError::bad_request("errors.csrf_missing"))?;

    // Token must match the one stored in the session at login time.
    let stored: String = session
        .get(SESSION_CSRF_TOKEN)
        .await
        .map_err(|e| ApiError::internal(Some(e.to_string())))?
        .ok_or_else(|| ApiError::bad_request("errors.csrf_missing"))?;

    if !constant_time_eq(header_token.as_bytes(), stored.as_bytes()) {
        return Err(ApiError::new(
            axum::http::StatusCode::FORBIDDEN,
            "errors.csrf_mismatch",
        ));
    }

    Ok(next.run(req).await)
}

/// Compare two byte strings without short-circuiting on the first mismatch.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::constant_time_eq;

    #[test]
    fn compares_by_value_and_length() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"", b""));
    }
}
