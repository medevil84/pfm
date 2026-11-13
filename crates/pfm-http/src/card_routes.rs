//! Credit-card settlement route handlers: catch up on all pending card
//! settlement cycles (or preview them) and report the card's remaining limit.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;

use pfm_service::CardError;

use crate::dto::{CardChargeDto, CardChargeRequest, CardForecastDto, CardForecastQuery};
use crate::error::ApiError;
use crate::session::AuthUserId;
use crate::state::AppState;

/// Build the credit-card settlement routes router.
pub fn card_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/accounts/{id}/card/catch-up", post(catch_up))
        .route("/cards/forecast", get(forecast))
}

impl From<CardError> for ApiError {
    fn from(e: CardError) -> Self {
        use pfm_service::CardError::*;
        match e {
            AccountNotFound => Self::not_found("errors.account_not_found"),
            Forbidden => Self::forbidden(),
            NotACard => Self::unprocessable("errors.not_a_card", None),
            MissingBacking => Self::unprocessable("errors.no_backing_account", None),
            Validation(msg) => Self::unprocessable("errors.validation", Some(msg)),
            Internal(msg) => Self::internal(Some(msg)),
        }
    }
}

/// `POST /accounts/{id}/card/catch-up`
async fn catch_up(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
    Json(body): Json<CardChargeRequest>,
) -> Result<Json<CardChargeDto>, ApiError> {
    // Materialize the account's own rule-defined movements (annual fees,
    // stamp duty, etc.) first, so they are included in the cycle total.
    // Previously only the periodic scheduler did this before settling; the
    // manual button skipped it, so rule rows never showed up when the user
    // settled by hand. Best-effort: a rule-generation failure should not
    // block the settlement preview/charge itself.
    if let Err(e) = state.rules.generate_all_for_account(&user_id, &id).await {
        tracing::warn!("card catch-up: rule generation for {id} failed: {e}");
    }

    let as_of = Utc::now().date_naive();
    let outcome = state
        .cards
        .catch_up_full(&user_id, &id, as_of, body.dry_run)
        .await?;
    Ok(Json(CardChargeDto::from(&outcome)))
}

/// `GET /cards/forecast?from&to` — expected charges of the cards' open and
/// closed-but-not-yet-charged cycles, for the register's forecast rows.
async fn forecast(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Query(q): Query<CardForecastQuery>,
) -> Result<Json<Vec<CardForecastDto>>, ApiError> {
    let parse = |s: &str| {
        chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map_err(|_| ApiError::bad_request("errors.invalid_date"))
    };
    let (from, to) = (parse(&q.from)?, parse(&q.to)?);
    if from > to {
        return Err(ApiError::bad_request("errors.date_range_order"));
    }
    let today = Utc::now().date_naive();
    let list = state.cards.forecast(&user_id, from, to, today).await?;
    Ok(Json(list.iter().map(CardForecastDto::from).collect()))
}
