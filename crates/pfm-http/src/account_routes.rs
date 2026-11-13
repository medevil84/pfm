//! Account CRUD route handlers.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::delete;
use axum::routing::get;
use axum::{Json, Router};
use chrono::NaiveDate;

use pfm_domain::{AccountKind, ShareLevel};
use pfm_service::accounts::{AccountCreateCmd, AccountUpdateCmd};
use pfm_service::{AccountError, VirtualError};

use crate::dto::{
    AccountCreateRequest, AccountDto, AccountListQueryParams, AccountReorderRequest,
    AccountUpdateRequest, BalanceQueryParams, BalanceResponse, ShareCreateRequest, ShareDto,
    ShareSearchResponse,
};
use crate::error::ApiError;
use crate::session::AuthUserId;
use crate::state::AppState;
use crate::validate::{
    check_cents, check_cents_opt, check_email, check_icon, validate_account_fields,
};

/// Build the account routes router.
pub fn account_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/accounts", get(list_accounts).post(create_account))
        // Static path `/accounts/reorder` must be registered before the dynamic
        // `/accounts/{id}` to prevent axum from treating "reorder" as an id.
        .route("/accounts/reorder", axum::routing::post(reorder_accounts))
        .route(
            "/accounts/{id}",
            get(get_account).put(update_account).delete(delete_account),
        )
        .route("/accounts/{id}/shares", get(list_shares).post(add_share))
        .route("/accounts/{id}/shares/{uid}", delete(remove_share))
        .route("/accounts/{id}/shares/search", get(search_share_email))
        .route("/accounts/{id}/balance", get(get_balance))
}

fn to_create_cmd(req: &AccountCreateRequest) -> Result<AccountCreateCmd, ApiError> {
    validate_account_fields(&req.name, &req.note, &req.iban)?;
    if let Some(ref c) = req.currency {
        if c.len() > 3 || !c.chars().all(|ch| ch.is_ascii_uppercase()) {
            return Err(ApiError::with_args(
                StatusCode::BAD_REQUEST,
                "errors.validation",
                format!("currency must be a 3-letter ISO code (got '{c}')"),
            ));
        }
    }
    check_cents(req.start_balance_cents)?;
    check_cents_opt(&req.limit_cents)?;
    check_cents_opt(&req.goal_cents)?;
    check_cents_opt(&req.min_balance_cents)?;
    check_icon(&req.icon)?;

    let kind = AccountKind::parse(&req.kind).ok_or_else(|| {
        ApiError::with_args(
            StatusCode::BAD_REQUEST,
            "errors.validation",
            format!("invalid kind: {}", req.kind),
        )
    })?;

    let start_date = NaiveDate::parse_from_str(&req.start_date, "%Y-%m-%d")
        .map_err(|_| ApiError::bad_request("errors.invalid_date"))?;

    Ok(AccountCreateCmd {
        name: req.name.clone(),
        kind,
        icon: req.icon.clone(),
        note: req.note.clone(),
        iban: req.iban.clone(),
        currency: req.currency.clone(),
        start_date,
        start_balance_cents: req.start_balance_cents,
        limit_cents: req.limit_cents,
        charge_day: req.charge_day,
        closing_day: req.closing_day,
        primary_account_id: req.primary_account_id.clone(),
        rate_pct_bps: req.rate_pct_bps,
        goal_cents: req.goal_cents,
        min_balance_cents: req.min_balance_cents,
    })
}

fn to_update_cmd(req: &AccountUpdateRequest) -> Result<AccountUpdateCmd, ApiError> {
    if let Some(icon) = req.icon.as_deref() {
        check_icon(icon)?;
    }
    Ok(AccountUpdateCmd {
        name: req.name.clone(),
        icon: req.icon.clone(),
        note: req.note.clone(),
        iban: req.iban.clone(),
        limit_cents: req.limit_cents,
        charge_day: req.charge_day,
        closing_day: req.closing_day,
        primary_account_id: req.primary_account_id.clone(),
        rate_pct_bps: req.rate_pct_bps,
        goal_cents: req.goal_cents,
        min_balance_cents: req.min_balance_cents,
        active: req.active,
    })
}

impl From<AccountError> for ApiError {
    fn from(e: AccountError) -> Self {
        match e {
            AccountError::NotFound => Self::not_found("errors.account_not_found"),
            AccountError::Forbidden => Self::forbidden(),
            AccountError::Validation(msg) => Self::unprocessable("errors.validation", Some(msg)),
            AccountError::InUse => Self::conflict("errors.account_in_use"),
            AccountError::Internal(msg) => Self::internal(Some(msg)),
        }
    }
}

impl From<VirtualError> for ApiError {
    fn from(e: VirtualError) -> Self {
        match e {
            VirtualError::AccountNotFound => Self::not_found("errors.account_not_found"),
            VirtualError::Forbidden => Self::forbidden(),
            VirtualError::Internal(msg) => Self::internal(Some(msg)),
        }
    }
}

/// `GET /accounts`
async fn list_accounts(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Query(params): Query<AccountListQueryParams>,
) -> Result<Json<Vec<AccountDto>>, ApiError> {
    let accounts = state.accounts.list(&user_id).await?;

    let kind = match params.kind.as_deref() {
        Some(s) if !s.is_empty() => Some(AccountKind::parse(s).ok_or_else(|| {
            ApiError::with_args(
                StatusCode::BAD_REQUEST,
                "errors.validation",
                format!("invalid kind: {s}"),
            )
        })?),
        _ => None,
    };

    let dtos = accounts
        .iter()
        .filter(|a| kind.map_or(true, |k| a.kind == k))
        .map(AccountDto::from)
        .collect();
    Ok(Json(dtos))
}

/// `POST /accounts`
async fn create_account(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Json(body): Json<AccountCreateRequest>,
) -> Result<Response, ApiError> {
    let cmd = to_create_cmd(&body)?;
    let account = state.accounts.create(&user_id, &cmd).await?;
    let dto = AccountDto::from(&account);
    Ok((StatusCode::CREATED, Json(dto)).into_response())
}

/// `GET /accounts/{id}`
async fn get_account(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
) -> Result<Json<AccountDto>, ApiError> {
    let account = state.accounts.get(&user_id, &id).await?;
    Ok(Json(AccountDto::from(&account)))
}

/// `PUT /accounts/{id}`
async fn update_account(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
    Json(body): Json<AccountUpdateRequest>,
) -> Result<Json<AccountDto>, ApiError> {
    let cmd = to_update_cmd(&body)?;
    let account = state.accounts.update(&user_id, &id, &cmd).await?;
    Ok(Json(AccountDto::from(&account)))
}

/// `POST /accounts/reorder`
///
/// Accepts an ordered list of account ids and sets `sort_order = index` for
/// each account that the caller owns. Shared / foreign accounts in the list
/// are silently skipped.
async fn reorder_accounts(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Json(body): Json<AccountReorderRequest>,
) -> Result<axum::http::StatusCode, ApiError> {
    state.accounts.reorder(&user_id, &body.ids).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// `DELETE /accounts/{id}`
async fn delete_account(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    state.accounts.delete(&user_id, &id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `GET /accounts/{id}/balance?vacct_included=...`
///
/// Returns the account's two balances: the bank balance (`real_cents`, which
/// ignores every set-aside) and the available balance (`allocated_cents`, which
/// folds back the allocations of the piggy banks listed in `vacct_included`).
async fn get_balance(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
    Query(params): Query<BalanceQueryParams>,
) -> Result<Json<BalanceResponse>, ApiError> {
    let view = state
        .virtuals
        .balance(&user_id, &id, &params.vacct_included.0)
        .await?;
    let mut resp = BalanceResponse::from(&view);
    let (avail, outstanding) = state
        .cards
        .outstanding_for(&user_id, &id)
        .await
        .map_err(ApiError::from)?;
    resp.card_available_cents = avail;
    resp.card_outstanding_cents = outstanding;
    Ok(Json(resp))
}

// ─── Share handlers ────────────────────────────────────────────────────────────

/// `GET /accounts/{id}/shares`
async fn list_shares(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
) -> Result<Json<Vec<ShareDto>>, ApiError> {
    let shares = state.sharing.list_shares(&user_id, &id).await?;
    Ok(Json(shares.iter().map(ShareDto::from).collect()))
}

/// `POST /accounts/{id}/shares`
async fn add_share(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
    Json(body): Json<ShareCreateRequest>,
) -> Result<Json<ShareDto>, ApiError> {
    check_email(&body.email)?;
    let level = match body.level.as_deref() {
        Some(s) => ShareLevel::parse(s).ok_or_else(|| {
            ApiError::with_args(
                StatusCode::BAD_REQUEST,
                "errors.validation",
                format!("invalid level: {s}"),
            )
        })?,
        None => ShareLevel::Read,
    };
    let entry = state
        .sharing
        .add_share(&user_id, &id, &body.email, level)
        .await?;
    Ok(Json(ShareDto::from(&entry)))
}

/// `DELETE /accounts/{id}/shares/{uid}`
async fn remove_share(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path((id, uid)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    state.sharing.remove_share(&user_id, &id, &uid).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `GET /accounts/{id}/shares/search?email=...`
async fn search_share_email(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
    Query(params): Query<SearchParams>,
) -> Result<Json<ShareSearchResponse>, ApiError> {
    let shared = state
        .sharing
        .search_email(&user_id, &id, &params.email)
        .await?;
    Ok(Json(ShareSearchResponse { shared }))
}

#[derive(Debug, serde::Deserialize)]
struct SearchParams {
    email: String,
}

impl From<pfm_service::ShareError> for ApiError {
    fn from(e: pfm_service::ShareError) -> Self {
        match e {
            pfm_service::ShareError::AccountNotFound => Self::not_found("errors.account_not_found"),
            pfm_service::ShareError::UserNotFound => Self::not_found("errors.user_not_found"),
            pfm_service::ShareError::Forbidden => Self::forbidden(),
            pfm_service::ShareError::Validation(msg) => {
                Self::unprocessable("errors.validation", Some(msg))
            }
            pfm_service::ShareError::Internal(msg) => Self::internal(Some(msg)),
        }
    }
}
