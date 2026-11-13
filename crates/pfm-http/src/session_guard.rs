//! Per-request session revalidation.
//!
//! The session cookie only carries a signed session id, and the session store
//! is memory-only, so on its own a session stays valid until it expires — even
//! if the user was disabled, deleted, or had their password changed in the
//! meantime. This middleware closes that gap: for every API request that
//! carries an authenticated session it re-reads the user and ends the session
//! (401) when the user is gone or inactive, or when the password hash is no
//! longer the one the session was opened with.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use tower_sessions::Session;

use crate::error::ApiError;
use crate::session::{password_fingerprint, SESSION_PW_FINGERPRINT, SESSION_USER_ID};
use crate::state::AppState;

/// Paths that must not be gated: the login itself (it replaces the session) and
/// the setup wizard (no user exists yet).
fn is_exempt(path: &str) -> bool {
    let logical = path.strip_prefix("/api").unwrap_or(path);
    logical == "/auth/login" || logical.starts_with("/setup/")
}

pub async fn session_guard(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let path = req.uri().path();
    // Static assets and probes never need a user lookup.
    if !path.starts_with("/api/") || is_exempt(path) {
        return Ok(next.run(req).await);
    }

    let Some(session) = req.extensions().get::<Session>().cloned() else {
        return Ok(next.run(req).await);
    };

    let user_id: Option<String> = session.get(SESSION_USER_ID).await.ok().flatten();
    let Some(user_id) = user_id else {
        return Ok(next.run(req).await);
    };

    let user = state
        .auth
        .get_user_by_id(&user_id)
        .await
        .map_err(|e| ApiError::internal(Some(e.to_string())))?;

    let still_valid = match user {
        Some(u) if u.active => {
            let stored: Option<String> = session.get(SESSION_PW_FINGERPRINT).await.ok().flatten();
            stored.as_deref() == Some(password_fingerprint(&u.password_hash).as_str())
        }
        _ => false,
    };

    if !still_valid {
        let _ = session.flush().await;
        return Err(ApiError::unauthorized());
    }

    Ok(next.run(req).await)
}

/// Re-bind the *current* session to the user's present password hash, so the
/// session that performed a password change stays logged in while every other
/// session of that user is ended.
pub async fn refresh_session_fingerprint(state: &AppState, session: &Session, user_id: &str) {
    if let Ok(Some(user)) = state.auth.get_user_by_id(user_id).await {
        let _ = session
            .insert(
                SESSION_PW_FINGERPRINT,
                password_fingerprint(&user.password_hash),
            )
            .await;
    }
}
