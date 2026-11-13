//! Admin-only middleware: ensures the authenticated user has the `Admin` role.
//!
//! Applied to the `/users` and `/admin/*` route groups. Returns 403 for
//! non-admin users and 401 for unauthenticated requests.

use std::sync::Arc;

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use tower_sessions::Session;

use crate::error::ApiError;
use crate::session::SESSION_USER_ID;
use crate::state::AppState;

/// Middleware that checks the session user is an admin.
///
/// On success, injects the session into extensions so downstream handlers
/// can read the impersonation key.
pub async fn admin_guard(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let session = req
        .extensions_mut()
        .get_mut::<Session>()
        .cloned()
        .ok_or_else(ApiError::unauthorized)?;

    let user_id: String = session
        .get(SESSION_USER_ID)
        .await
        .map_err(|_| ApiError::unauthorized())?
        .ok_or_else(ApiError::unauthorized)?;

    // Verify the user is an admin.
    let user = state
        .auth
        .get_user_by_id(&user_id)
        .await
        .map_err(|e| ApiError::internal(Some(e.to_string())))?
        .ok_or_else(|| ApiError::not_found("errors.user_not_found"))?;

    if user.role != pfm_domain::Role::Admin {
        return Err(ApiError::new(
            axum::http::StatusCode::FORBIDDEN,
            "errors.admin_role_required",
        ));
    }

    // Re-inject session so handlers can access impersonation state.
    req.extensions_mut().insert(session);
    req.extensions_mut().insert(Arc::new(user_id));

    let response = next.run(req).await;
    Ok(response)
}
