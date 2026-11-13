//! CSV import route handlers: preview, commit, and profile listing.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use pfm_domain::{ColumnRole, DateFormat, DecimalSep};
use pfm_service::import::ImportParams;
use pfm_service::ImportError;

use crate::dto::{
    ImportCommitResponse, ImportPreviewResponse, ImportProfileDto, ImportProfilesResponse,
    ImportRequest,
};
use crate::error::ApiError;
use crate::session::AuthUserId;
use crate::state::AppState;

/// Build the import routes router.
pub fn import_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/import/preview", axum::routing::post(preview))
        .route("/import/commit", axum::routing::post(commit))
        .route("/import/profiles", get(list_profiles).post(create_profile))
}

/// Convert an `ImportRequest` into an `ImportParams`, parsing the optional
/// enum fields.
fn to_params(req: &ImportRequest) -> Result<ImportParams, ApiError> {
    let delimiter = match &req.delimiter {
        Some(d) if d.len() == 1 => d.chars().next().unwrap(),
        Some(d) => {
            return Err(ApiError::with_args(
                StatusCode::BAD_REQUEST,
                "errors.validation",
                format!("delimiter must be a single character, got '{d}'"),
            ))
        }
        None => ';', // default
    };

    let date_format = match &req.date_format {
        Some(df) => DateFormat::try_from(df).ok_or_else(|| {
            ApiError::with_args(
                StatusCode::BAD_REQUEST,
                "errors.validation",
                format!("invalid date_format '{df}'"),
            )
        })?,
        None => DateFormat::Dmy, // default
    };

    let decimal_sep = match &req.decimal_sep {
        Some(ds) => DecimalSep::try_from(ds).ok_or_else(|| {
            ApiError::with_args(
                StatusCode::BAD_REQUEST,
                "errors.validation",
                format!("invalid decimal_sep '{ds}'"),
            )
        })?,
        None => DecimalSep::Comma, // default
    };

    let col_map = match &req.col_map {
        Some(map) => {
            let mut roles = Vec::new();
            // Find the max key to size the vec.
            let max_idx = map.keys().filter_map(|k| k.parse::<usize>().ok()).max();
            let len = max_idx.map(|m| m + 1).unwrap_or(0);
            roles.resize(len, ColumnRole::Other);
            for (k, v) in map {
                let idx = k.parse::<usize>().map_err(|_| {
                    ApiError::with_args(
                        StatusCode::BAD_REQUEST,
                        "errors.validation",
                        format!("col_map key '{k}' is not a number"),
                    )
                })?;
                let role = ColumnRole::try_from(v).ok_or_else(|| {
                    ApiError::with_args(
                        StatusCode::BAD_REQUEST,
                        "errors.validation",
                        format!("invalid col_map role '{v}'"),
                    )
                })?;
                if idx < roles.len() {
                    roles[idx] = role;
                } else {
                    roles.resize(idx + 1, ColumnRole::Other);
                    roles[idx] = role;
                }
            }
            Some(roles)
        }
        None => None,
    };

    Ok(ImportParams {
        account_id: req.account_id.clone(),
        bank_name: req.bank_name.clone(),
        csv_text: req.csv_content.clone(),
        delimiter: Some(delimiter),
        date_format: Some(date_format),
        decimal_sep: Some(decimal_sep),
        col_map,
    })
}

impl From<ImportError> for ApiError {
    fn from(e: ImportError) -> Self {
        use ImportError::*;
        match e {
            AccountNotFound => Self::not_found("errors.account_not_found"),
            Forbidden => Self::forbidden(),
            ProfileNotFound => Self::not_found("errors.import_profile_not_found"),
            Parse(msg) => Self::with_args(
                axum::http::StatusCode::BAD_REQUEST,
                "errors.validation",
                msg,
            ),
            Internal(msg) => Self::internal(Some(msg)),
        }
    }
}

/// `POST /import/preview` – dry-run: parse the CSV, return classified rows.
/// Does not write anything to the database.
async fn preview(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Json(body): Json<ImportRequest>,
) -> Result<Json<ImportPreviewResponse>, ApiError> {
    let params = to_params(&body)?;
    let result = state.import.preview(&user_id, &params).await?;
    Ok(Json(ImportPreviewResponse::from(&result)))
}

/// `POST /import/commit` – write new rows in a single transaction,
/// idempotent via import_hash.
async fn commit(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Json(body): Json<ImportRequest>,
) -> Result<Response, ApiError> {
    let params = to_params(&body)?;
    let result = state.import.commit(&user_id, &params).await?;
    let dto = ImportCommitResponse::from(&result);
    Ok((StatusCode::OK, Json(dto)).into_response())
}

/// `GET /import/profiles?account={id}` – list import profiles.
async fn list_profiles(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    axum::extract::Query(params): axum::extract::Query<ProfileQueryParams>,
) -> Result<Json<ImportProfilesResponse>, ApiError> {
    let profiles = state
        .import
        .list_profiles(&user_id, params.account.as_deref())
        .await?;
    let dtos: Vec<ImportProfileDto> = profiles.iter().map(ImportProfileDto::from).collect();
    Ok(Json(ImportProfilesResponse { profiles: dtos }))
}

/// `POST /import/profiles` – create or update an import profile manually.
async fn create_profile(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Json(body): Json<CreateProfileRequest>,
) -> Result<Json<ImportProfileDto>, ApiError> {
    let profile = pfm_domain::ImportProfile {
        id: String::new(),
        account_id: body.account_id.clone(),
        bank_name: body.bank_name.clone(),
        delimiter: body
            .delimiter
            .chars()
            .next()
            .ok_or_else(|| ApiError::bad_request("errors.validation"))?,
        date_format: DateFormat::try_from(&body.date_format).ok_or_else(|| {
            ApiError::with_args(
                StatusCode::BAD_REQUEST,
                "errors.validation",
                format!("invalid date_format '{}'", body.date_format),
            )
        })?,
        decimal_sep: DecimalSep::try_from(&body.decimal_sep).ok_or_else(|| {
            ApiError::with_args(
                StatusCode::BAD_REQUEST,
                "errors.validation",
                format!("invalid decimal_sep '{}'", body.decimal_sep),
            )
        })?,
        col_map: None, // Manual col_map via POST is future work.
        auto_cat: body.auto_cat,
        skip_dup: body.skip_dup,
        created_at: String::new(),
    };

    // Use the service to save (it handles account access check).
    let params = ImportParams {
        account_id: body.account_id.clone(),
        bank_name: body.bank_name.clone(),
        csv_text: String::new(),
        delimiter: Some(profile.delimiter),
        date_format: Some(profile.date_format),
        decimal_sep: Some(profile.decimal_sep),
        col_map: None,
    };

    // Verify account access.
    let _ = state
        .import
        .list_profiles(&user_id, Some(&body.account_id))
        .await?;

    // Save the profile directly via the service's commit path (which
    // includes the access check). We use a minimal csv to trigger the
    // profile save.
    // Actually, let's just save the profile directly.
    // For now, we use the import service's internal profile save.
    // This is a bit of a workaround; a dedicated endpoint would be cleaner.
    let result = state
        .import
        .commit(&user_id, &params)
        .await
        .map_err(|e| match e {
            ImportError::Parse(msg) => {
                ApiError::with_args(StatusCode::BAD_REQUEST, "errors.validation", msg)
            }
            other => ApiError::from(other),
        })?;

    // Fetch the saved profile.
    let profiles = state
        .import
        .list_profiles(&user_id, Some(&body.account_id))
        .await?;
    let dto = profiles
        .iter()
        .find(|p| p.bank_name == body.bank_name)
        .map(ImportProfileDto::from)
        .ok_or_else(|| ApiError::not_found("errors.profile_not_found_after_save"))?;

    let _ = result; // commit result not needed here
    Ok(Json(dto))
}

/// Query parameters for `GET /import/profiles`.
#[derive(Debug, Default, Deserialize)]
struct ProfileQueryParams {
    #[serde(default)]
    account: Option<String>,
}

/// `POST /import/profiles` request body.
#[derive(Debug, Deserialize)]
struct CreateProfileRequest {
    pub account_id: String,
    pub bank_name: String,
    pub delimiter: String,
    pub date_format: String,
    pub decimal_sep: String,
    #[serde(default)]
    pub auto_cat: bool,
    #[serde(default)]
    pub skip_dup: bool,
}

use serde::Deserialize;
