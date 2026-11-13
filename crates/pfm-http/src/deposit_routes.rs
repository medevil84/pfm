//! Deposit-interest route handlers: preview the interest accrued over a period
//! and consolidate (credit) it onto the linked current account.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::routing::post;
use axum::{Json, Router};
use chrono::NaiveDate;

use pfm_service::DepositError;

use crate::dto::{ConsolidateRequest, ConsolidateResponse, InterestDto, InterestQueryParams};
use crate::error::ApiError;
use crate::session::AuthUserId;
use crate::state::AppState;

/// Build the deposit-interest routes router.
pub fn deposit_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/accounts/{id}/deposit/interest",
            axum::routing::get(preview_interest),
        )
        .route(
            "/accounts/{id}/deposit/consolidate",
            post(consolidate_interest),
        )
}

impl From<DepositError> for ApiError {
    fn from(e: DepositError) -> Self {
        use pfm_service::DepositError::*;
        match e {
            AccountNotFound => Self::not_found("errors.account_not_found"),
            Forbidden => Self::forbidden(),
            NotADeposit => Self::unprocessable("errors.not_a_deposit", None),
            MissingBacking => Self::unprocessable("errors.no_backing_account", None),
            MissingRate => Self::unprocessable("errors.no_rate_set", None),
            Validation(msg) => Self::unprocessable("errors.validation", Some(msg)),
            Internal(msg) => Self::internal(Some(msg)),
        }
    }
}

fn parse_date(s: &str) -> Result<NaiveDate, ApiError> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| ApiError::bad_request("errors.invalid_date"))
}

/// `GET /accounts/{id}/deposit/interest?from=...&to=...`
async fn preview_interest(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
    Query(params): Query<InterestQueryParams>,
) -> Result<Json<InterestDto>, ApiError> {
    let from = parse_date(&params.from)?;
    let to = parse_date(&params.to)?;
    let result = state
        .deposits
        .preview_interest(&user_id, &id, from, to)
        .await?;
    Ok(Json(InterestDto::from(&result)))
}

/// `POST /accounts/{id}/deposit/consolidate`
async fn consolidate_interest(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
    Json(body): Json<ConsolidateRequest>,
) -> Result<Json<ConsolidateResponse>, ApiError> {
    let from = parse_date(&body.from)?;
    let to = parse_date(&body.to)?;
    let outcome = state
        .deposits
        .consolidate_interest(&user_id, &id, from, to, body.dry_run)
        .await?;
    Ok(Json(ConsolidateResponse::from(&outcome)))
}
