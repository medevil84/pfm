//! Recurrence rule route handlers: CRUD, runtime occurrence generation, and
//! per-date override confirmation.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::NaiveDate;

use pfm_domain::{CategoryKind, EndType, RecurrenceMode, WeekendPolicy};
use pfm_service::{OverrideCmd, RecurrenceCreateCmd, RecurrenceUpdateCmd, StaleAction};

use crate::dto::{
    DateImpactDto, OccurrencesQueryParams, OverrideDto, OverrideRequest, RecurrenceCreateRequest,
    RecurrenceDto, RecurrenceOccurrenceDto, RecurrenceUpdateRequest, RecurrenceUpdateResponse,
};
use crate::error::ApiError;
use crate::session::AuthUserId;
use crate::state::AppState;

/// Build the recurrence routes router.
pub fn recurrence_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/recurrences",
            get(list_recurrences).post(create_recurrence),
        )
        .route(
            "/recurrences/{id}",
            get(get_recurrence)
                .put(update_recurrence)
                .delete(delete_recurrence),
        )
        .route("/recurrences/{id}/occurrences", get(get_occurrences))
        .route("/recurrences/{id}/date-impact", post(get_date_impact))
        .route("/recurrences/{id}/overrides/{date}", put(set_override))
}

fn parse_date(s: &str) -> Result<NaiveDate, ApiError> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| ApiError::bad_request("errors.invalid_date"))
}

fn to_create_cmd(req: &RecurrenceCreateRequest) -> Result<RecurrenceCreateCmd, ApiError> {
    let kind = CategoryKind::parse(&req.kind).ok_or_else(|| {
        ApiError::with_args(
            StatusCode::BAD_REQUEST,
            "errors.validation",
            format!("invalid kind: {}", req.kind),
        )
    })?;
    let weekend = match req.weekend.as_deref() {
        Some(s) => WeekendPolicy::parse(s).ok_or_else(|| {
            ApiError::with_args(
                StatusCode::BAD_REQUEST,
                "errors.validation",
                format!("invalid weekend policy: {s}"),
            )
        })?,
        None => WeekendPolicy::Create,
    };
    let end_type = match req.end_type.as_deref() {
        Some(s) => EndType::parse(s).ok_or_else(|| {
            ApiError::with_args(
                StatusCode::BAD_REQUEST,
                "errors.validation",
                format!("invalid end_type: {s}"),
            )
        })?,
        None => EndType::Never,
    };
    let mode = match req.mode.as_deref() {
        Some(s) => RecurrenceMode::parse(s).ok_or_else(|| {
            ApiError::with_args(
                StatusCode::BAD_REQUEST,
                "errors.validation",
                format!("invalid mode: {s}"),
            )
        })?,
        None => RecurrenceMode::Fixed,
    };
    let first_date = parse_date(&req.first_date)?;
    let end_date = match req.end_date.as_deref() {
        Some(s) => Some(parse_date(s)?),
        None => None,
    };

    Ok(RecurrenceCreateCmd {
        description: req.description.clone(),
        category_id: req.category_id.clone(),
        kind,
        amount_cents: req.amount_cents,
        account_id: req.account_id.clone(),
        pattern: req.pattern.clone(),
        weekend,
        first_date,
        end_type,
        end_date,
        end_count: req.end_count,
        mode,
        avg_n: req.avg_n.unwrap_or(1),
        active: req.active.unwrap_or(true),
        note: req.note.clone().unwrap_or_default(),
        linked_account_id: req.linked_account_id.clone(),
        linked_category_id: req.linked_category_id.clone(),
    })
}

fn to_update_cmd(req: &RecurrenceUpdateRequest) -> Result<RecurrenceUpdateCmd, ApiError> {
    let weekend = req
        .weekend
        .as_deref()
        .map(|s| {
            WeekendPolicy::parse(s).ok_or_else(|| {
                ApiError::with_args(
                    StatusCode::BAD_REQUEST,
                    "errors.validation",
                    format!("invalid weekend policy: {s}"),
                )
            })
        })
        .transpose()?;
    let end_type = req
        .end_type
        .as_deref()
        .map(|s| {
            EndType::parse(s).ok_or_else(|| {
                ApiError::with_args(
                    StatusCode::BAD_REQUEST,
                    "errors.validation",
                    format!("invalid end_type: {s}"),
                )
            })
        })
        .transpose()?;
    let mode = req
        .mode
        .as_deref()
        .map(|s| {
            RecurrenceMode::parse(s).ok_or_else(|| {
                ApiError::with_args(
                    StatusCode::BAD_REQUEST,
                    "errors.validation",
                    format!("invalid mode: {s}"),
                )
            })
        })
        .transpose()?;
    let first_date = match req.first_date.as_deref() {
        Some(s) => Some(parse_date(s)?),
        None => None,
    };
    let end_date = match &req.end_date {
        Some(inner) => Some(match inner.as_deref() {
            Some(s) => Some(parse_date(s)?),
            None => None,
        }),
        None => None,
    };

    let apply_from = match req.apply_from.as_deref() {
        Some(s) => Some(parse_date(s)?),
        None => None,
    };

    let stale_action = match req.stale_action.as_deref() {
        None | Some("keep") => StaleAction::Keep,
        Some("delete_auto") => StaleAction::DeleteAuto,
        Some("delete_all") => StaleAction::DeleteAll,
        Some(s) => {
            return Err(ApiError::with_args(
                StatusCode::BAD_REQUEST,
                "errors.validation",
                format!("invalid stale_action: {s}"),
            ))
        }
    };

    Ok(RecurrenceUpdateCmd {
        create_missing: req.create_missing,
        stale_action,
        apply_to_existing: req.apply_to_existing,
        apply_from,
        overwrite_manual: req.overwrite_manual,
        description: req.description.clone(),
        category_id: req.category_id.clone(),
        amount_cents: req.amount_cents,
        account_id: req.account_id.clone(),
        pattern: req.pattern.clone(),
        weekend,
        first_date,
        end_type,
        end_date,
        end_count: req.end_count,
        mode,
        avg_n: req.avg_n,
        active: req.active,
        note: req.note.clone(),
        linked_account_id: req.linked_account_id.clone(),
        linked_category_id: req.linked_category_id.clone(),
    })
}

impl From<pfm_service::RecurrenceError> for ApiError {
    fn from(e: pfm_service::RecurrenceError) -> Self {
        use pfm_service::RecurrenceError::*;
        match e {
            NotFound => Self::not_found("errors.recurrence_not_found"),
            AccountNotFound => Self::not_found("errors.account_not_found"),
            Forbidden => Self::forbidden(),
            Validation(msg) => Self::unprocessable("errors.validation", Some(msg)),
            Internal(msg) => Self::internal(Some(msg)),
        }
    }
}

/// `GET /recurrences`
async fn list_recurrences(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
) -> Result<Json<Vec<RecurrenceDto>>, ApiError> {
    let recs = state.recurrences.list_for_owner(&user_id, &user_id).await?;
    Ok(Json(recs.iter().map(RecurrenceDto::from).collect()))
}

/// `POST /recurrences`
async fn create_recurrence(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Json(body): Json<RecurrenceCreateRequest>,
) -> Result<Response, ApiError> {
    let cmd = to_create_cmd(&body)?;
    let rec = state.recurrences.create(&user_id, &user_id, &cmd).await?;
    let dto = RecurrenceDto::from(&rec);
    Ok((StatusCode::CREATED, Json(dto)).into_response())
}

/// `GET /recurrences/{id}`
async fn get_recurrence(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
) -> Result<Json<RecurrenceDto>, ApiError> {
    let rec = state.recurrences.get(&user_id, &id).await?;
    Ok(Json(RecurrenceDto::from(&rec)))
}

/// `PUT /recurrences/{id}`
async fn update_recurrence(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
    Json(body): Json<RecurrenceUpdateRequest>,
) -> Result<Json<RecurrenceUpdateResponse>, ApiError> {
    let cmd = to_update_cmd(&body)?;
    let (rec, report) = state
        .recurrences
        .update_with_report(&user_id, &id, &cmd)
        .await?;
    Ok(Json(RecurrenceUpdateResponse {
        recurrence: RecurrenceDto::from(&rec),
        aligned: report.aligned,
        skipped_manual: report.skipped_manual,
        removed: report.removed,
        created: report.created,
    }))
}

/// `DELETE /recurrences/{id}`
async fn delete_recurrence(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    state.recurrences.delete(&user_id, &id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `GET /recurrences/{id}/occurrences?from=YYYY-MM-DD&to=YYYY-MM-DD`
async fn get_occurrences(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
    Query(params): Query<OccurrencesQueryParams>,
) -> Result<Json<Vec<RecurrenceOccurrenceDto>>, ApiError> {
    let from = parse_date(&params.from)?;
    let to = parse_date(&params.to)?;
    let occs = state
        .recurrences
        .occurrences(&user_id, &id, from, to)
        .await?;
    Ok(Json(
        occs.iter().map(RecurrenceOccurrenceDto::from).collect(),
    ))
}

/// `PUT /recurrences/{id}/overrides/{date}`
async fn set_override(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path((id, date)): Path<(String, String)>,
    Json(body): Json<OverrideRequest>,
) -> Result<Json<OverrideDto>, ApiError> {
    let date = parse_date(&date)?;
    let cmd = OverrideCmd {
        amount_cents: body.amount_cents,
        description: body.description.clone(),
        update_rule: body.update_rule,
    };
    let ov = state
        .recurrences
        .set_override(&user_id, &id, date, &cmd)
        .await?;
    Ok(Json(OverrideDto::from(&ov)))
}

/// `POST /recurrences/{id}/date-impact` — same body as the update; nothing is written.
async fn get_date_impact(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Path(id): Path<String>,
    Json(body): Json<RecurrenceUpdateRequest>,
) -> Result<Json<DateImpactDto>, ApiError> {
    let cmd = to_update_cmd(&body)?;
    let i = state.recurrences.date_impact(&user_id, &id, &cmd).await?;
    Ok(Json(DateImpactDto {
        to_create: i.to_create,
        stale_auto: i.stale_auto,
        stale_manual: i.stale_manual,
    }))
}
