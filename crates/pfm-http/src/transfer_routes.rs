//! Transfer and bulk-operation route handlers.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::NaiveDate;

use pfm_service::bulk::BulkAction;
use pfm_service::transfers::{TransferCreateCmd, TransferUpdateCmd};
use pfm_service::BulkError;
use pfm_service::TransferError;

use crate::dto::{
    BulkRequest, BulkResponse, TransferCreateRequest, TransferDto, TransferListQueryParams,
    TransferUpdateRequest,
};
use crate::error::ApiError;
use crate::session::AuthUserId;
use crate::state::AppState;
use crate::validate::{check_cents, validate_tx_fields};

/// Build the transfer + bulk routes router.
pub fn transfer_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/transfers", get(list_transfers).post(create_transfer))
        .route(
            "/transfers/{id}",
            get(get_transfer)
                .put(update_transfer)
                .delete(delete_transfer),
        )
        .route("/transactions/bulk", post(bulk_transactions))
}

fn to_create_cmd(req: &TransferCreateRequest) -> Result<TransferCreateCmd, ApiError> {
    let date = NaiveDate::parse_from_str(&req.date, "%Y-%m-%d")
        .map_err(|_| ApiError::bad_request("errors.invalid_date"))?;
    validate_tx_fields(&req.description, &req.note, req.cents)?;

    Ok(TransferCreateCmd {
        from_account_id: req.from_account_id.clone(),
        to_account_id: req.to_account_id.clone(),
        date,
        cents: req.cents,
        description: req.description.clone(),
        note: req.note.clone(),
    })
}

fn to_update_cmd(req: &TransferUpdateRequest) -> Result<TransferUpdateCmd, ApiError> {
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

    Ok(TransferUpdateCmd {
        from_account_id: req.from_account_id.clone(),
        to_account_id: req.to_account_id.clone(),
        date,
        cents: req.cents,
        description: req.description.clone(),
        note: req.note.clone(),
    })
}

impl From<TransferError> for ApiError {
    fn from(e: TransferError) -> Self {
        match e {
            TransferError::NotFound => Self::not_found("errors.transfer_not_found"),
            TransferError::AccountNotFound => Self::not_found("errors.account_not_found"),
            TransferError::Forbidden => Self::forbidden(),
            TransferError::Validation(msg) => Self::unprocessable("errors.validation", Some(msg)),
            TransferError::Internal(msg) => Self::internal(Some(msg)),
        }
    }
}

impl From<BulkError> for ApiError {
    fn from(e: BulkError) -> Self {
        match e {
            BulkError::Forbidden => Self::forbidden(),
            BulkError::Validation(msg) => Self::unprocessable("errors.validation", Some(msg)),
            BulkError::Internal(msg) => Self::internal(Some(msg)),
        }
    }
}

/// `GET /transfers?from&to&from_account&to_account`
async fn list_transfers(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Query(params): Query<TransferListQueryParams>,
) -> Result<Json<Vec<TransferDto>>, ApiError> {
    let from = NaiveDate::parse_from_str(&params.from, "%Y-%m-%d")
        .map_err(|_| ApiError::bad_request("errors.invalid_date"))?;
    let to = NaiveDate::parse_from_str(&params.to, "%Y-%m-%d")
        .map_err(|_| ApiError::bad_request("errors.invalid_date"))?;

    if from > to {
        return Err(ApiError::bad_request("errors.date_range_order"));
    }

    let transfers = state
        .transfers
        .list(
            &user_id,
            from,
            to,
            params.from_account.as_deref(),
            params.to_account.as_deref(),
        )
        .await?;

    Ok(Json(transfers.iter().map(TransferDto::from).collect()))
}

/// `POST /transfers`
async fn create_transfer(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Json(body): Json<TransferCreateRequest>,
) -> Result<Response, ApiError> {
    let cmd = to_create_cmd(&body)?;
    let transfer = state.transfers.create(&user_id, &cmd).await?;
    let dto = TransferDto::from(&transfer);
    Ok((StatusCode::CREATED, Json(dto)).into_response())
}

/// `GET /transfers/{id}`
async fn get_transfer(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
) -> Result<Json<TransferDto>, ApiError> {
    let transfer = state.transfers.get(&user_id, &id).await?;
    Ok(Json(TransferDto::from(&transfer)))
}

/// `PUT /transfers/{id}`
async fn update_transfer(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
    Json(body): Json<TransferUpdateRequest>,
) -> Result<Json<TransferDto>, ApiError> {
    let cmd = to_update_cmd(&body)?;
    let transfer = state.transfers.update(&user_id, &id, &cmd).await?;
    Ok(Json(TransferDto::from(&transfer)))
}

/// `DELETE /transfers/{id}`
async fn delete_transfer(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    state.transfers.delete(&user_id, &id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `POST /transactions/bulk`
async fn bulk_transactions(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Json(body): Json<BulkRequest>,
) -> Result<Json<BulkResponse>, ApiError> {
    let action = match body.action.as_str() {
        "delete" => BulkAction::Delete,
        "recategorize" => BulkAction::Recategorize,
        other => {
            return Err(ApiError::with_args(
                StatusCode::BAD_REQUEST,
                "errors.validation",
                format!("action must be 'delete' or 'recategorize', got '{other}'"),
            ));
        }
    };

    let affected = match action {
        BulkAction::Delete => state.bulk.bulk_delete(&user_id, &body.ids).await?,
        BulkAction::Recategorize => {
            state
                .bulk
                .bulk_recategorize(&user_id, &body.ids, body.category_id.as_deref())
                .await?
        }
    };

    Ok(Json(BulkResponse { affected }))
}
