//! Admin-only routes: user CRUD, impersonation, and global account listing.
//!
//! All endpoints in this module are guarded by the `admin_guard` middleware
//! which ensures the session user has the `Admin` role.

use std::sync::Arc;

use axum::extract::{FromRequestParts, Path, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use tower_sessions::Session;

use pfm_domain::Role;
use pfm_service::{AccountReassignment, TransactionReassignment, UserCreateCmd, UserUpdateCmd};

use crate::error::ApiError;
use crate::session::{AuthUserId, SESSION_IMPERSONATED_USER_ID, SESSION_USER_ID};
use crate::state::AppState;
use crate::validate::{check_email, check_len, check_password, MAX_USERNAME_LEN};

/// Build the admin routes router.
///
/// The caller must wrap this router with the `admin_guard` middleware.
pub fn admin_routes() -> Router<Arc<AppState>> {
    Router::new()
        // User CRUD
        .route("/users", get(list_users).post(create_user))
        .route(
            "/users/{id}",
            get(get_user).put(update_user).delete(delete_user),
        )
        // Impersonation
        .route("/admin/impersonate/{uid}", post(start_impersonation))
        .route("/admin/impersonate/stop", post(stop_impersonation))
        // Admin account listing
        .route("/admin/accounts", get(list_all_accounts))
        // Admin account deletion (with reassignment)
        .route("/admin/accounts/{id}", delete(delete_admin_account))
}

/// Extractor that provides the real admin actor id and the optional
/// impersonated user id from the session.
pub struct AdminActor {
    /// The real admin user id (the logged-in user).
    pub actor_id: String,
    /// The user being impersonated, if any.
    pub impersonated_as: Option<String>,
}

impl<R> FromRequestParts<R> for AdminActor
where
    R: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &R) -> Result<Self, Self::Rejection> {
        let session = Session::from_request_parts(parts, _state)
            .await
            .map_err(|_| ApiError::unauthorized())?;

        let actor_id: String = session
            .get(SESSION_USER_ID)
            .await
            .map_err(|_| ApiError::unauthorized())?
            .ok_or_else(ApiError::unauthorized)?;

        let impersonated_as = session
            .get::<String>(SESSION_IMPERSONATED_USER_ID)
            .await
            .ok()
            .flatten();

        Ok(Self {
            actor_id,
            impersonated_as,
        })
    }
}

// ─── User CRUD handlers ────────────────────────────────────────────────────────

/// `GET /users`
async fn list_users(
    State(state): State<Arc<AppState>>,
    AuthUserId(_actor_id): AuthUserId,
) -> Result<Json<Vec<crate::dto::AdminUserDto>>, ApiError> {
    let users = state.user_admin.list_users().await?;
    let dtos: Vec<crate::dto::AdminUserDto> = users.iter().map(Into::into).collect();
    Ok(Json(dtos))
}

/// `POST /users`
async fn create_user(
    State(state): State<Arc<AppState>>,
    actor: AdminActor,
    Json(body): Json<crate::dto::UserCreateRequest>,
) -> Result<Response, ApiError> {
    check_len(&body.username, MAX_USERNAME_LEN, "username")?;
    check_email(&body.email)?;
    check_password(&body.password)?;
    let role = match body.role.as_deref() {
        Some(r) => Role::parse(r).ok_or_else(|| {
            ApiError::with_args(
                StatusCode::BAD_REQUEST,
                "errors.validation",
                format!("invalid role: {r}"),
            )
        })?,
        None => Role::User,
    };

    let cmd = UserCreateCmd {
        username: body.username,
        email: body.email,
        password: body.password,
        role,
        locale: body.locale.unwrap_or_else(|| "it".into()),
        default_currency: body.default_currency.unwrap_or_else(|| "EUR".into()),
    };

    let user = state
        .user_admin
        .create_user(&actor.actor_id, &cmd, actor.impersonated_as.as_deref())
        .await?;

    let dto = crate::dto::AdminUserDto::from(&user);
    Ok((StatusCode::CREATED, Json(dto)).into_response())
}

/// `GET /users/{id}`
async fn get_user(
    State(state): State<Arc<AppState>>,
    AuthUserId(_actor_id): AuthUserId,
    Path(id): Path<String>,
) -> Result<Json<crate::dto::AdminUserDto>, ApiError> {
    let user = state.user_admin.get_user(&id).await?;
    Ok(Json(crate::dto::AdminUserDto::from(&user)))
}

/// `PUT /users/{id}`
async fn update_user(
    State(state): State<Arc<AppState>>,
    actor: AdminActor,
    session: Session,
    Path(id): Path<String>,
    Json(body): Json<crate::dto::UserUpdateRequest>,
) -> Result<Json<crate::dto::AdminUserDto>, ApiError> {
    if let Some(pw) = body.password.as_deref() {
        check_password(pw)?;
    }
    let resets_own_password = body.password.is_some() && id == actor.actor_id;
    let cmd = UserUpdateCmd {
        role: body.role.as_deref().and_then(Role::parse),
        active: body.active,
        locale: body.locale,
        default_currency: body.default_currency,
        password: body.password,
    };

    let user = state
        .user_admin
        .update_user(&actor.actor_id, &id, &cmd, actor.impersonated_as.as_deref())
        .await?;
    // A reset ends the target's other sessions (see `session_guard`); an admin
    // resetting their own password keeps the session they are using.
    if resets_own_password {
        crate::session_guard::refresh_session_fingerprint(&state, &session, &actor.actor_id).await;
    }
    Ok(Json(crate::dto::AdminUserDto::from(&user)))
}

/// `DELETE /users/{id}`
async fn delete_user(
    State(state): State<Arc<AppState>>,
    actor: AdminActor,
    Path(id): Path<String>,
    Json(body): Json<crate::dto::DeleteUserRequest>,
) -> Result<StatusCode, ApiError> {
    let reassignment = match &body.reassign_to {
        Some(target) => AccountReassignment::ReassignTo(target.clone()),
        None => AccountReassignment::Block,
    };

    state
        .user_admin
        .delete_user(
            &actor.actor_id,
            &id,
            &reassignment,
            actor.impersonated_as.as_deref(),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ─── Impersonation handlers ───────────────────────────────────────────────────

/// `POST /admin/impersonate/{uid}`
async fn start_impersonation(
    State(state): State<Arc<AppState>>,
    actor: AdminActor,
    session: Session,
    Path(uid): Path<String>,
) -> Result<Json<crate::dto::ImpersonationResponse>, ApiError> {
    let target = state.impersonation.start(&actor.actor_id, &uid).await?;

    // Store in session
    let _ = session
        .insert(SESSION_IMPERSONATED_USER_ID, target.clone())
        .await;

    Ok(Json(crate::dto::ImpersonationResponse {
        impersonating: Some(target),
    }))
}

/// `POST /admin/impersonate/stop`
async fn stop_impersonation(
    State(state): State<Arc<AppState>>,
    actor: AdminActor,
    session: Session,
) -> Result<StatusCode, ApiError> {
    let impersonated: Option<String> = session
        .get(SESSION_IMPERSONATED_USER_ID)
        .await
        .ok()
        .flatten();

    if let Some(imp) = impersonated {
        state.impersonation.stop(&actor.actor_id, &imp).await?;
        let _ = session.remove::<String>(SESSION_IMPERSONATED_USER_ID).await;
    }

    Ok(StatusCode::NO_CONTENT)
}

// ─── Admin account listing ─────────────────────────────────────────────────────

/// `GET /admin/accounts`
async fn list_all_accounts(
    State(state): State<Arc<AppState>>,
    AuthUserId(_actor_id): AuthUserId,
) -> Result<Json<Vec<crate::dto::AdminAccountDto>>, ApiError> {
    let accounts = state.user_admin.list_all_accounts().await?;
    let dtos: Vec<crate::dto::AdminAccountDto> = accounts.iter().map(Into::into).collect();
    Ok(Json(dtos))
}

/// `DELETE /admin/accounts/{id}`
async fn delete_admin_account(
    State(state): State<Arc<AppState>>,
    actor: AdminActor,
    Path(id): Path<String>,
    Json(body): Json<crate::dto::DeleteAccountRequest>,
) -> Result<StatusCode, ApiError> {
    let reassignment = match &body.reassign_tx_to {
        Some(target) => TransactionReassignment::ReassignTo(target.clone()),
        None => TransactionReassignment::Block,
    };

    state
        .user_admin
        .delete_account(
            &actor.actor_id,
            &id,
            &reassignment,
            actor.impersonated_as.as_deref(),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
