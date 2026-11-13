//! Category CRUD route handlers.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};

use pfm_domain::CategoryKind;
use pfm_service::categories::{CategoryCreateCmd, CategoryUpdateCmd};
use pfm_service::CategoryError;

use crate::dto::{CategoryCreateRequest, CategoryDto, CategoryUpdateRequest};
use crate::error::ApiError;
use crate::session::AuthUserId;
use crate::state::AppState;
use crate::validate::check_emoji;

/// Build the category routes router.
pub fn category_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/categories", get(list_categories).post(create_category))
        .route(
            "/categories/{id}",
            get(get_category)
                .put(update_category)
                .delete(delete_category),
        )
}

fn to_create_cmd(req: &CategoryCreateRequest) -> Result<CategoryCreateCmd, ApiError> {
    let kind = CategoryKind::parse(&req.kind).ok_or_else(|| {
        ApiError::with_args(
            StatusCode::BAD_REQUEST,
            "errors.validation",
            format!("invalid kind: {}", req.kind),
        )
    })?;

    check_emoji(&req.emoji)?;

    Ok(CategoryCreateCmd {
        name: req.name.clone(),
        emoji: req.emoji.clone(),
        kind,
        budget_cents: req.budget_cents,
    })
}

fn to_update_cmd(req: &CategoryUpdateRequest) -> Result<CategoryUpdateCmd, ApiError> {
    if let Some(emoji) = req.emoji.as_deref() {
        check_emoji(emoji)?;
    }
    Ok(CategoryUpdateCmd {
        name: req.name.clone(),
        emoji: req.emoji.clone(),
        budget_cents: req.budget_cents,
    })
}

impl From<CategoryError> for ApiError {
    fn from(e: CategoryError) -> Self {
        match e {
            CategoryError::NotFound => Self::not_found("errors.category_not_found"),
            CategoryError::Validation(msg) => Self::unprocessable("errors.validation", Some(msg)),
            CategoryError::FallbackProtected => {
                Self::conflict("errors.fallback_category_protected")
            }
            CategoryError::Forbidden => Self::forbidden(),
            CategoryError::Duplicate => Self::conflict("errors.category_exists"),
            CategoryError::InUse => Self::conflict("errors.category_in_use"),
            CategoryError::Internal(msg) => Self::internal(Some(msg)),
        }
    }
}

/// `GET /categories`
async fn list_categories(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
) -> Result<Json<Vec<CategoryDto>>, ApiError> {
    let categories = state.categories.list(&user_id).await?;
    Ok(Json(categories.iter().map(CategoryDto::from).collect()))
}

/// `POST /categories`
async fn create_category(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Json(body): Json<CategoryCreateRequest>,
) -> Result<Response, ApiError> {
    let cmd = to_create_cmd(&body)?;
    let category = state.categories.create(&user_id, &cmd).await?;
    let dto = CategoryDto::from(&category);
    Ok((StatusCode::CREATED, Json(dto)).into_response())
}

/// `GET /categories/{id}`
async fn get_category(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
) -> Result<Json<CategoryDto>, ApiError> {
    let category = state.categories.get(&user_id, &id).await?;
    Ok(Json(CategoryDto::from(&category)))
}

/// `PUT /categories/{id}`
async fn update_category(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
    Json(body): Json<CategoryUpdateRequest>,
) -> Result<Json<CategoryDto>, ApiError> {
    let cmd = to_update_cmd(&body)?;
    let category = state.categories.update(&user_id, &id, &cmd).await?;
    Ok(Json(CategoryDto::from(&category)))
}

/// `DELETE /categories/{id}`
async fn delete_category(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    state.categories.delete(&user_id, &id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
