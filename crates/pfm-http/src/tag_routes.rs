//! Tag route handlers: tag CRUD and assigning tags to transactions.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use pfm_service::{TagCmd, TagError};

use crate::error::ApiError;
use crate::session::AuthUserId;
use crate::state::AppState;

pub fn tag_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/tags", get(list_tags).post(create_tag))
        .route("/tags/assignments", get(assignments))
        .route("/tags/{id}", put(update_tag).delete(delete_tag))
        .route("/transactions/{id}/tags", put(set_transaction_tags))
        .route("/transactions/bulk-tags", post(bulk_tags))
}

#[derive(Serialize)]
struct TagDto {
    id: String,
    label: String,
    description: String,
    color: String,
    /// Number of transactions carrying the tag (list endpoint only).
    uses: i64,
    /// Creator of the tag; absent for shared ones.
    #[serde(skip_serializing_if = "Option::is_none")]
    owner_id: Option<String>,
    /// Whether the requesting user may edit / delete it.
    editable: bool,
}

impl TagDto {
    fn new(t: &pfm_repo::Tag, uses: i64, editable: bool) -> Self {
        Self {
            id: t.id.clone(),
            label: t.label.clone(),
            description: t.description.clone(),
            color: t.color.clone(),
            uses,
            owner_id: t.owner_id.clone(),
            editable,
        }
    }
}

#[derive(Deserialize)]
struct TagRequest {
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    color: Option<String>,
}

impl From<TagRequest> for TagCmd {
    fn from(r: TagRequest) -> Self {
        TagCmd {
            label: r.label,
            description: r.description,
            color: r.color,
        }
    }
}

#[derive(Deserialize)]
struct SetTagsRequest {
    tag_ids: Vec<String>,
}

#[derive(Deserialize)]
struct BulkTagsRequest {
    ids: Vec<String>,
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    remove: Vec<String>,
}

impl From<TagError> for ApiError {
    fn from(e: TagError) -> Self {
        match e {
            TagError::NotFound => Self::not_found("errors.tag_not_found"),
            TagError::Duplicate => Self::conflict("errors.tag_duplicate"),
            TagError::Forbidden => Self::forbidden(),
            TagError::Validation(msg) => Self::unprocessable("errors.validation", Some(msg)),
            TagError::Internal(msg) => Self::internal(Some(msg)),
        }
    }
}

/// `GET /tags`
async fn list_tags(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
) -> Result<Json<Vec<TagDto>>, ApiError> {
    let tags = state.tags.list(&user_id).await?;
    Ok(Json(
        tags.iter()
            .map(|i| TagDto::new(&i.tag, i.uses, i.editable))
            .collect(),
    ))
}

/// `POST /tags`
async fn create_tag(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Json(body): Json<TagRequest>,
) -> Result<Response, ApiError> {
    let tag = state.tags.create(&user_id, &body.into()).await?;
    Ok((StatusCode::CREATED, Json(TagDto::new(&tag, 0, true))).into_response())
}

/// `PUT /tags/{id}`
async fn update_tag(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
    Json(body): Json<TagRequest>,
) -> Result<Json<TagDto>, ApiError> {
    let tag = state.tags.update(&user_id, &id, &body.into()).await?;
    Ok(Json(TagDto::new(&tag, 0, true)))
}

/// `DELETE /tags/{id}`
async fn delete_tag(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    state.tags.delete(&user_id, &id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `GET /tags/assignments` — `{ transaction_id: [tag_id, ...] }` for every
/// transaction the caller can see.
async fn assignments(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
) -> Result<Json<BTreeMap<String, Vec<String>>>, ApiError> {
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (tx, tag) in state.tags.assignments(&user_id).await? {
        map.entry(tx).or_default().push(tag);
    }
    Ok(Json(map))
}

/// `PUT /transactions/{id}/tags`
async fn set_transaction_tags(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
    Json(body): Json<SetTagsRequest>,
) -> Result<StatusCode, ApiError> {
    state
        .tags
        .set_for_transaction(&user_id, &id, &body.tag_ids)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /transactions/bulk-tags`
async fn bulk_tags(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Json(body): Json<BulkTagsRequest>,
) -> Result<StatusCode, ApiError> {
    state
        .tags
        .bulk_change(&user_id, &body.ids, &body.add, &body.remove)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
