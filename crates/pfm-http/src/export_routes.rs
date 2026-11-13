//! `POST /export` handler: returns a fixed-format CSV file for the register.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use chrono::NaiveDate;

use pfm_repo::RegisterFilters;
use pfm_service::ExportError;

use crate::dto::RegisterQueryParams;
use crate::error::ApiError;
use crate::session::AuthUserId;
use crate::state::AppState;

/// Build the export routes router.
pub fn export_routes() -> Router<Arc<AppState>> {
    Router::new().route("/export", post(export_handler))
}

/// Convert query params into the shared `RegisterFilters` struct.
fn to_filters(params: &RegisterQueryParams) -> RegisterFilters {
    RegisterFilters {
        category_id: params.cat.clone(),
        tx_type: params.tipo.clone(),
        min_cents: params.min,
        max_cents: params.max,
        text: params.q.clone(),
        vacct_included: Some(params.vacct_included.0.clone()),
    }
}

impl From<ExportError> for ApiError {
    fn from(e: ExportError) -> Self {
        match e {
            ExportError::AccountNotFound => Self::not_found("errors.account_not_found"),
            ExportError::Forbidden => Self::forbidden(),
            ExportError::Validation(msg) => Self::unprocessable("errors.validation", Some(msg)),
            ExportError::Internal(msg) => Self::internal(Some(msg)),
        }
    }
}

/// `POST /export?from&to&account&cat&tipo&min&max&q&vacct_included`
///
/// Returns a CSV file with `Content-Type: text/csv; charset=utf-8` and
/// `Content-Disposition: attachment; filename="…"`.
async fn export_handler(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Query(params): Query<RegisterQueryParams>,
) -> Result<Response, ApiError> {
    let from = NaiveDate::parse_from_str(&params.from, "%Y-%m-%d")
        .map_err(|_| ApiError::bad_request("errors.invalid_date"))?;
    let to = NaiveDate::parse_from_str(&params.to, "%Y-%m-%d")
        .map_err(|_| ApiError::bad_request("errors.invalid_date"))?;

    let filters = to_filters(&params);
    let account_id = params.account.as_deref().unwrap_or("");
    let output = state
        .export
        .export(&user_id, account_id, from, to, &filters)
        .await?;

    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        "text/csv; charset=utf-8".parse().unwrap(),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{}\"", output.filename)
            .parse()
            .unwrap(),
    );
    headers.insert(
        header::CONTENT_LENGTH,
        output.csv.len().to_string().parse().unwrap(),
    );

    Ok((StatusCode::OK, headers, output.csv).into_response())
}
