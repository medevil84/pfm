//! Spending-rule route handlers: CRUD under an account, fee-row generation, and
//! read-only preview.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::NaiveDate;

use pfm_domain::{Cadence, CalcType, CondBase, CondOp};
use pfm_service::{RuleCreateCmd, RulePreview, RuleRangeCmd, RuleUpdateCmd};

use crate::dto::{
    FeeRowDto, RuleCreateRequest, RuleDto, RuleGenerateRequest, RulePreviewDto, RulePreviewRequest,
    RuleUpdateRequest,
};
use crate::error::ApiError;
use crate::session::AuthUserId;
use crate::state::AppState;

/// Build the spending-rule routes router.
pub fn rule_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/accounts/{id}/rules", get(list_rules).post(create_rule))
        .route(
            "/accounts/{id}/rules/{rid}",
            get(get_rule).put(update_rule).delete(delete_rule),
        )
        .route("/accounts/{id}/rules/{rid}/generate", post(generate_rule))
        .route("/accounts/{id}/rules/preview", post(preview_rules))
}

fn parse_date(s: &str) -> Result<NaiveDate, ApiError> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| ApiError::bad_request("errors.invalid_date"))
}

fn to_range_cmd(from: Option<&str>, to: Option<&str>) -> Result<RuleRangeCmd, ApiError> {
    let from = match from {
        Some(s) => Some(parse_date(s)?),
        None => None,
    };
    let to = match to {
        Some(s) => Some(parse_date(s)?),
        None => None,
    };
    Ok(RuleRangeCmd { from, to })
}

fn to_create_cmd(req: &RuleCreateRequest) -> Result<RuleCreateCmd, ApiError> {
    let calc_type = CalcType::parse(&req.calc_type).ok_or_else(|| {
        ApiError::with_args(
            StatusCode::BAD_REQUEST,
            "errors.validation",
            format!("invalid calc_type: {}", req.calc_type),
        )
    })?;
    let cadence = Cadence::parse(&req.cadence).ok_or_else(|| {
        ApiError::with_args(
            StatusCode::BAD_REQUEST,
            "errors.validation",
            format!("invalid cadence: {}", req.cadence),
        )
    })?;
    let cond_base = match req.cond_base.as_deref() {
        Some(s) => CondBase::parse(s).ok_or_else(|| {
            ApiError::with_args(
                StatusCode::BAD_REQUEST,
                "errors.validation",
                format!("invalid cond_base: {s}"),
            )
        })?,
        None => CondBase::Always,
    };
    let cond_op = match req.cond_op.as_deref() {
        Some(s) => Some(CondOp::parse(s).ok_or_else(|| {
            ApiError::with_args(
                StatusCode::BAD_REQUEST,
                "errors.validation",
                format!("invalid cond_op: {s}"),
            )
        })?),
        None => None,
    };

    Ok(RuleCreateCmd {
        name: req.name.clone(),
        category_id: req.category_id.clone(),
        calc_type,
        value: req.value,
        is_annual: req.is_annual,
        prorata_days: req.prorata_days,
        annual_cap_cents: req.annual_cap_cents,
        cadence,
        charge_day: req.charge_day.unwrap_or(0),
        charge_month: req.charge_month,
        cond_base,
        cond_op,
        cond_threshold: req.cond_threshold,
        active: req.active.unwrap_or(true),
    })
}

fn to_update_cmd(req: &RuleUpdateRequest) -> Result<RuleUpdateCmd, ApiError> {
    let calc_type = req
        .calc_type
        .as_deref()
        .map(|s| {
            CalcType::parse(s).ok_or_else(|| {
                ApiError::with_args(
                    StatusCode::BAD_REQUEST,
                    "errors.validation",
                    format!("invalid calc_type: {s}"),
                )
            })
        })
        .transpose()?;
    let cadence = req
        .cadence
        .as_deref()
        .map(|s| {
            Cadence::parse(s).ok_or_else(|| {
                ApiError::with_args(
                    StatusCode::BAD_REQUEST,
                    "errors.validation",
                    format!("invalid cadence: {s}"),
                )
            })
        })
        .transpose()?;
    let cond_base = req
        .cond_base
        .as_deref()
        .map(|s| {
            CondBase::parse(s).ok_or_else(|| {
                ApiError::with_args(
                    StatusCode::BAD_REQUEST,
                    "errors.validation",
                    format!("invalid cond_base: {s}"),
                )
            })
        })
        .transpose()?;
    let cond_op = match &req.cond_op {
        Some(inner) => Some(match inner.as_deref() {
            Some(s) => Some(CondOp::parse(s).ok_or_else(|| {
                ApiError::with_args(
                    StatusCode::BAD_REQUEST,
                    "errors.validation",
                    format!("invalid cond_op: {s}"),
                )
            })?),
            None => None,
        }),
        None => None,
    };

    Ok(RuleUpdateCmd {
        name: req.name.clone(),
        category_id: req.category_id.clone(),
        calc_type,
        value: req.value,
        is_annual: req.is_annual,
        prorata_days: req.prorata_days,
        annual_cap_cents: req.annual_cap_cents,
        cadence,
        charge_day: req.charge_day,
        charge_month: req.charge_month,
        cond_base,
        cond_op,
        cond_threshold: req.cond_threshold,
        active: req.active,
    })
}

impl From<pfm_service::FeeRuleError> for ApiError {
    fn from(e: pfm_service::FeeRuleError) -> Self {
        use pfm_service::FeeRuleError::*;
        match e {
            NotFound => Self::not_found("errors.rule_not_found"),
            AccountNotFound => Self::not_found("errors.account_not_found"),
            Forbidden => Self::forbidden(),
            Validation(msg) => Self::unprocessable("errors.validation", Some(msg)),
            Internal(msg) => Self::internal(Some(msg)),
        }
    }
}

fn to_preview_dto(p: &RulePreview) -> RulePreviewDto {
    RulePreviewDto {
        rule_id: p.rule_id.clone(),
        name: p.name.clone(),
        rows: p.rows.iter().map(FeeRowDto::from).collect(),
    }
}

/// `GET /accounts/{id}/rules`
async fn list_rules(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
) -> Result<Json<Vec<RuleDto>>, ApiError> {
    let rules = state.rules.list_for_account(&user_id, &id).await?;
    Ok(Json(rules.iter().map(RuleDto::from).collect()))
}

/// `POST /accounts/{id}/rules`
async fn create_rule(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
    Json(body): Json<RuleCreateRequest>,
) -> Result<Response, ApiError> {
    let cmd = to_create_cmd(&body)?;
    let rule = state.rules.create(&user_id, &id, &cmd).await?;
    let dto = RuleDto::from(&rule);
    Ok((StatusCode::CREATED, Json(dto)).into_response())
}

/// `GET /accounts/{id}/rules/{rid}`
async fn get_rule(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path((id, rid)): Path<(String, String)>,
) -> Result<Json<RuleDto>, ApiError> {
    let rule = state.rules.get(&user_id, &id, &rid).await?;
    Ok(Json(RuleDto::from(&rule)))
}

/// `PUT /accounts/{id}/rules/{rid}`
async fn update_rule(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path((id, rid)): Path<(String, String)>,
    Json(body): Json<RuleUpdateRequest>,
) -> Result<Json<RuleDto>, ApiError> {
    let cmd = to_update_cmd(&body)?;
    let rule = state.rules.update(&user_id, &id, &rid, &cmd).await?;
    Ok(Json(RuleDto::from(&rule)))
}

/// `DELETE /accounts/{id}/rules/{rid}`
async fn delete_rule(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path((id, rid)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    state.rules.delete(&user_id, &id, &rid).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `POST /accounts/{id}/rules/{rid}/generate`
async fn generate_rule(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path((id, rid)): Path<(String, String)>,
    Json(body): Json<RuleGenerateRequest>,
) -> Result<Json<Vec<FeeRowDto>>, ApiError> {
    let cmd = to_range_cmd(body.from.as_deref(), body.to.as_deref())?;
    let rows = state.rules.generate(&user_id, &id, &rid, &cmd).await?;
    Ok(Json(rows.iter().map(FeeRowDto::from).collect()))
}

/// `POST /accounts/{id}/rules/preview`
async fn preview_rules(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
    Json(body): Json<RulePreviewRequest>,
) -> Result<Json<Vec<RulePreviewDto>>, ApiError> {
    let cmd = to_range_cmd(body.from.as_deref(), body.to.as_deref())?;
    let previews = state
        .rules
        .preview_rules(&user_id, &id, body.rule_id.as_deref(), &cmd)
        .await?;
    Ok(Json(previews.iter().map(to_preview_dto).collect()))
}
