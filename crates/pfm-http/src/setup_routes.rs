//! `/api/setup/*` routes – available ONLY when the process was started with
//! `--setup`. In normal mode these routes are not registered at all.
//!
//! Endpoints:
//!   * `GET  /api/setup/state`   – report whether the DB is configured.
//!   * `POST /api/setup/reset`   – wipe all application data.
//!   * `POST /api/setup/complete` – create admin user + prefs, mark done.
//!   * `POST /api/setup/cancel`  – abort setup and shut the process down.
//!
//! These routes do NOT use the standard session/CSRF middleware because the
//! wizard creates the first user (there is no prior login). They are exempt
//! from CSRF validation (see `csrf.rs` `CSRF_EXEMPT`) and are instead gated by
//! a one-time token (`X-Setup-Token`) that the process prints on its terminal
//! at startup; without it every call is refused with 401.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

/// GET /api/setup/state
#[derive(Debug, Serialize)]
pub struct SetupStateResponse {
    /// `true` when the DB has been configured (marker + admin present).
    pub configured: bool,
    /// Human-readable hint for the UI.
    pub hint: String,
}

/// POST /api/setup/complete – request body.
#[derive(Debug, Deserialize)]
pub struct CompleteRequest {
    pub username: String,
    pub password: String,
    pub email: String,
    #[serde(default = "default_app_name")]
    pub app_name: String,
    #[serde(default = "default_locale")]
    pub locale: String,
    #[serde(default = "default_currency")]
    pub currency: String,
}

fn default_app_name() -> String {
    "PFM".to_string()
}
fn default_locale() -> String {
    "it".to_string()
}
fn default_currency() -> String {
    "EUR".to_string()
}

/// POST /api/setup/complete – response body.
#[derive(Debug, Serialize)]
pub struct CompleteResponse {
    /// Always `true` on success; the UI should show the termination dialog.
    pub success: bool,
    /// Message shown to the user before the process exits.
    pub message: String,
}

/// POST /api/setup/reset – response body.
#[derive(Debug, Serialize)]
pub struct ResetResponse {
    pub success: bool,
    pub message: String,
}

/// POST /api/setup/cancel – response body.
#[derive(Debug, Serialize)]
pub struct CancelResponse {
    pub success: bool,
    pub message: String,
}

/// Build the `/api/setup/*` router, guarded by `token`.
///
/// The caller mounts this on the main router ONLY in `--setup` mode. In
/// normal mode these routes simply do not exist.
pub fn setup_routes(token: Arc<String>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/setup/state", axum::routing::get(get_state))
        .route("/api/setup/reset", post(reset_data))
        .route("/api/setup/complete", post(complete_setup))
        .route("/api/setup/cancel", post(cancel_setup))
        // `route_layer`: only the four setup routes are gated; unmatched paths
        // keep falling through to the static-asset fallback.
        .route_layer(axum::middleware::from_fn_with_state(token, require_token))
}

/// Header carrying the one-time setup token.
pub const SETUP_TOKEN_HEADER: &str = "x-setup-token";

async fn require_token(
    State(token): State<Arc<String>>,
    req: Request,
    next: Next,
) -> Result<Response, (StatusCode, String)> {
    let given = req
        .headers()
        .get(SETUP_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !constant_time_eq(given.as_bytes(), token.as_bytes()) {
        return Err((
            StatusCode::UNAUTHORIZED,
            "setup token missing or wrong".to_string(),
        ));
    }
    Ok(next.run(req).await)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Log the real cause of a failure and return a fixed message: database /
/// driver text must not reach the client.
fn internal_failure(context: &str, e: impl std::fmt::Display) -> (StatusCode, String) {
    tracing::error!("{context}: {e}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal error".to_string(),
    )
}

/// GET /api/setup/state
async fn get_state(
    State(state): State<Arc<AppState>>,
) -> Result<Json<SetupStateResponse>, (StatusCode, String)> {
    let configured = state
        .setup
        .is_configured()
        .await
        .map_err(|e| internal_failure("setup state", e))?;
    let hint = if configured {
        "Database is configured. Running --setup will reset all data."
    } else {
        "Database is not yet configured. Complete the setup wizard."
    };
    Ok(Json(SetupStateResponse {
        configured,
        hint: hint.to_string(),
    }))
}

/// POST /api/setup/reset
async fn reset_data(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ResetResponse>, (StatusCode, String)> {
    state
        .setup
        .reset_all_data()
        .await
        .map_err(|e| internal_failure("setup reset", e))?;
    Ok(Json(ResetResponse {
        success: true,
        message: "All data has been reset. The setup wizard can now be run.".to_string(),
    }))
}

/// POST /api/setup/cancel
///
/// Aborting the wizard has to terminate the process, not just repaint the page.
/// The reset page used to tell the user to close the browser window, on the
/// assumption that the server would exit when the client disconnected — it does
/// not. The process stayed alive in setup mode with `/api/setup/reset` reachable
/// without authentication or CSRF, which is the one state where that endpoint is
/// genuinely dangerous.
async fn cancel_setup(
    State(state): State<Arc<AppState>>,
) -> Result<Json<CancelResponse>, (StatusCode, String)> {
    if let Some(notify) = state.shutdown_notify.as_ref() {
        notify.notify_one();
    } else {
        tracing::warn!("setup cancelled but no shutdown handle is available");
    }

    Ok(Json(CancelResponse {
        success: true,
        message: "Setup cancelled. The application is shutting down.".to_string(),
    }))
}

/// POST /api/setup/complete
async fn complete_setup(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CompleteRequest>,
) -> Result<Json<CompleteResponse>, (StatusCode, String)> {
    let params = pfm_service::SetupCompleteParams {
        username: req.username,
        password: req.password,
        email: req.email,
        app_name: req.app_name,
        locale: req.locale,
        currency: req.currency,
    };

    // Creating the first admin is only legitimate on an unconfigured
    // database. After an initialised install the operator has to go through
    // the explicit reset step first.
    if state
        .setup
        .is_configured()
        .await
        .map_err(|e| internal_failure("setup state", e))?
    {
        return Err((
            StatusCode::CONFLICT,
            "already configured: reset the data first".to_string(),
        ));
    }

    state.setup.complete(&params).await.map_err(|e| match e {
        pfm_service::SetupError::Internal(_) | pfm_service::SetupError::ResetFailed(_) => {
            internal_failure("setup complete", e)
        }
        // Validation / conflict messages are fixed strings meant for the form.
        other => (StatusCode::UNPROCESSABLE_ENTITY, other.to_string()),
    })?;

    // Trigger graceful shutdown: the process should exit after this response
    // is delivered to the client.
    if let Some(notify) = state.shutdown_notify.as_ref() {
        notify.notify_one();
    }

    Ok(Json(CompleteResponse {
        success: true,
        message: "Setup complete. The application is shutting down.".to_string(),
    }))
}
