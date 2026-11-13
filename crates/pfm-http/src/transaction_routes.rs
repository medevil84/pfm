//! Transaction CRUD and register route handlers.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use pfm_domain::RegisterResult;
use pfm_repo::RegisterFilters;
use pfm_service::transactions::{TransactionCreateCmd, TransactionUpdateCmd};
use pfm_service::TransactionError;

use crate::dto::{
    RegisterQueryParams, RegisterResponse, RegisterSummaryDto, TransactionCreateRequest,
    TransactionDto, TransactionUpdateRequest,
};
use crate::error::ApiError;
use crate::session::AuthUserId;
use crate::state::AppState;
use crate::validate::{check_cents, validate_tx_fields};

/// Build the transaction routes router.
pub fn transaction_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/transactions",
            get(list_transactions).post(create_transaction),
        )
        .route(
            "/transactions/{id}",
            get(get_transaction)
                .put(update_transaction)
                .delete(delete_transaction),
        )
        .route("/transactions/by-pair/{pair_id}", get(get_by_pair))
        .route("/transactions/suggest", get(suggest_descriptions))
}

/// Minimum characters before the server is asked for description suggestions.
const SUGGEST_MIN_CHARS: usize = 3;
const SUGGEST_LIMIT: usize = 8;

#[derive(Debug, Deserialize)]
struct SuggestParams {
    #[serde(default)]
    q: String,
    /// `-1` = outflows only, `1` = inflows only; absent = both.
    #[serde(default)]
    sign: Option<i8>,
}

#[derive(Serialize)]
struct SuggestionDto {
    description: String,
    cents: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    category_id: Option<String>,
    count: u32,
}

/// `GET /transactions/suggest?q=` — description autocomplete, each entry
/// carrying the amount and category of its most recent use.
async fn suggest_descriptions(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Query(params): Query<SuggestParams>,
) -> Result<Json<Vec<SuggestionDto>>, ApiError> {
    let q = params.q.trim();
    if q.chars().count() < SUGGEST_MIN_CHARS {
        return Ok(Json(vec![]));
    }
    let found = state
        .transactions
        .suggest_descriptions(&user_id, q, params.sign, SUGGEST_LIMIT)
        .await?;
    Ok(Json(
        found
            .into_iter()
            .map(|s| SuggestionDto {
                description: s.description,
                cents: s.cents,
                category_id: s.category_id,
                count: s.count,
            })
            .collect(),
    ))
}

fn to_create_cmd(req: &TransactionCreateRequest) -> Result<TransactionCreateCmd, ApiError> {
    let date = NaiveDate::parse_from_str(&req.date, "%Y-%m-%d")
        .map_err(|_| ApiError::bad_request("errors.invalid_date"))?;
    validate_tx_fields(&req.description, &req.note, req.cents)?;

    Ok(TransactionCreateCmd {
        account_id: req.account_id.clone(),
        date,
        cents: req.cents,
        description: req.description.clone(),
        category_id: req.category_id.clone(),
        note: req.note.clone(),
        client_id: req.client_id.clone(),
    })
}

fn to_update_cmd(req: &TransactionUpdateRequest) -> Result<TransactionUpdateCmd, ApiError> {
    let date = match &req.date {
        Some(d) => Some(
            NaiveDate::parse_from_str(d, "%Y-%m-%d")
                .map_err(|_| ApiError::bad_request("errors.invalid_date"))?,
        ),
        None => None,
    };
    if let Some(c) = req.cents {
        check_cents(c)?;
    }

    Ok(TransactionUpdateCmd {
        date,
        cents: req.cents,
        description: req.description.clone(),
        category_id: req.category_id.clone(),
        note: req.note.clone(),
        account_id: req.account_id.clone(),
    })
}

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

impl From<TransactionError> for ApiError {
    fn from(e: TransactionError) -> Self {
        match e {
            TransactionError::NotFound => Self::not_found("errors.transaction_not_found"),
            TransactionError::AccountNotFound => Self::not_found("errors.account_not_found"),
            TransactionError::Forbidden => Self::forbidden(),
            TransactionError::Validation(msg) => {
                Self::unprocessable("errors.validation", Some(msg))
            }
            TransactionError::Internal(msg) => Self::internal(Some(msg)),
        }
    }
}

/// `GET /transactions?from&to&account&cat&tipo&min&max&q`
async fn list_transactions(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Query(params): Query<RegisterQueryParams>,
) -> Result<Json<RegisterResponse>, ApiError> {
    let from = NaiveDate::parse_from_str(&params.from, "%Y-%m-%d")
        .map_err(|_| ApiError::bad_request("errors.invalid_date"))?;
    let to = NaiveDate::parse_from_str(&params.to, "%Y-%m-%d")
        .map_err(|_| ApiError::bad_request("errors.invalid_date"))?;

    if from > to {
        return Err(ApiError::bad_request("errors.date_range_order"));
    }

    // No account specified (e.g. user has no accounts yet): return empty register.
    let account_id = match &params.account {
        Some(a) if !a.is_empty() => a.clone(),
        _ => {
            return Ok(Json(RegisterResponse {
                days: vec![],
                summary: RegisterSummaryDto {
                    income_cents: 0,
                    expense_cents: 0,
                    net_cents: 0,
                    opening_cents: 0,
                    closing_cents: 0,
                },
            }));
        }
    };

    let filters = to_filters(&params);
    let result: RegisterResult = state
        .transactions
        .register(&user_id, &account_id, from, to, &filters)
        .await?;
    Ok(Json(RegisterResponse::from(&result)))
}

/// `POST /transactions`
async fn create_transaction(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Json(body): Json<TransactionCreateRequest>,
) -> Result<Response, ApiError> {
    let cmd = to_create_cmd(&body)?;
    let tx = state.transactions.create(&user_id, &cmd).await?;
    let dto = TransactionDto::from(&tx);
    Ok((StatusCode::CREATED, Json(dto)).into_response())
}

/// `GET /transactions/{id}`
async fn get_transaction(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
) -> Result<Json<TransactionDto>, ApiError> {
    let tx = state.transactions.get(&user_id, &id).await?;
    Ok(Json(TransactionDto::from(&tx)))
}

/// `PUT /transactions/{id}`
async fn update_transaction(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
    Json(body): Json<TransactionUpdateRequest>,
) -> Result<Json<TransactionDto>, ApiError> {
    let cmd = to_update_cmd(&body)?;
    let tx = state.transactions.update(&user_id, &id, &cmd).await?;
    Ok(Json(TransactionDto::from(&tx)))
}

/// `DELETE /transactions/{id}`
async fn delete_transaction(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    state.transactions.delete(&user_id, &id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `GET /transactions/by-pair/{pair_id}`
async fn get_by_pair(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(pair_id): Path<String>,
) -> Result<Json<Vec<TransactionDto>>, ApiError> {
    let legs = state.transactions.get_by_pair(&user_id, &pair_id).await?;
    let dtos = legs.iter().map(TransactionDto::from).collect();
    Ok(Json(dtos))
}
