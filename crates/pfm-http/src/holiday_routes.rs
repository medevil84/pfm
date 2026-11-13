//! Holiday calendar routes. Reading calendars is open to any signed-in user (it
//! feeds the calendar selector in their settings); every write is admin-only and
//! lives in [`holiday_admin_routes`], which the caller wraps with `admin_guard`.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use pfm_domain::RuleParts;
use pfm_service::{HolidayError, HolidayService};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::session::AuthUserId;
use crate::state::AppState;

pub fn holiday_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/holiday-calendars", get(list_calendars))
        .route("/holiday-calendars/{code}/rules", get(get_rules))
        .route("/holiday-calendars/{code}/preview", get(preview_saved))
}

/// Admin-only writes; wrap with `admin_guard`.
pub fn holiday_admin_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin/holiday-calendars", post(create_calendar))
        .route("/admin/holiday-calendars/preview", post(preview_rules))
        .route(
            "/admin/holiday-calendars/{code}",
            put(rename_calendar).delete(delete_calendar),
        )
        .route("/admin/holiday-calendars/{code}/rules", put(replace_rules))
}

#[derive(Serialize)]
struct CalendarDto {
    code: String,
    name: String,
}

#[derive(Serialize, Deserialize)]
struct RuleDto {
    label: String,
    kind: String,
    #[serde(default)]
    month: Option<i32>,
    #[serde(default)]
    day: Option<i32>,
    #[serde(default)]
    offset_days: Option<i32>,
    #[serde(default)]
    weekday: Option<i32>,
    #[serde(default)]
    nth: Option<i32>,
    #[serde(default)]
    observance: Option<String>,
    #[serde(default)]
    date: Option<String>,
}

impl From<RuleParts> for RuleDto {
    fn from(p: RuleParts) -> Self {
        Self {
            label: p.label,
            kind: p.kind,
            month: p.month,
            day: p.day,
            offset_days: p.offset_days,
            weekday: p.weekday,
            nth: p.nth,
            observance: p.observance,
            date: p.date,
        }
    }
}

impl From<RuleDto> for RuleParts {
    fn from(d: RuleDto) -> Self {
        Self {
            label: d.label,
            kind: d.kind,
            month: d.month,
            day: d.day,
            offset_days: d.offset_days,
            weekday: d.weekday,
            nth: d.nth,
            observance: d.observance,
            date: d.date,
        }
    }
}

#[derive(Serialize)]
struct HolidayDto {
    date: String,
    label: String,
}

#[derive(Deserialize)]
struct CreateRequest {
    code: String,
    name: String,
}

#[derive(Deserialize)]
struct RenameRequest {
    name: String,
}

#[derive(Deserialize)]
struct RulesRequest {
    rules: Vec<RuleDto>,
}

#[derive(Deserialize)]
struct PreviewRequest {
    rules: Vec<RuleDto>,
    year: i32,
}

#[derive(Deserialize)]
struct YearQuery {
    year: i32,
}

impl From<HolidayError> for ApiError {
    fn from(e: HolidayError) -> Self {
        match e {
            HolidayError::NotFound => Self::not_found("errors.holiday_calendar_not_found"),
            HolidayError::Duplicate => Self::conflict("errors.holiday_calendar_duplicate"),
            HolidayError::InUse(_) => Self::conflict("errors.holiday_calendar_in_use"),
            HolidayError::Validation(msg) => Self::unprocessable("errors.validation", Some(msg)),
            HolidayError::Internal(msg) => Self::internal(Some(msg)),
        }
    }
}

fn holidays_dto(list: Vec<(chrono::NaiveDate, String)>) -> Vec<HolidayDto> {
    list.into_iter()
        .map(|(d, label)| HolidayDto {
            date: d.format("%Y-%m-%d").to_string(),
            label,
        })
        .collect()
}

async fn list_calendars(
    State(state): State<Arc<AppState>>,
    _auth: AuthUserId,
) -> Result<Json<Vec<CalendarDto>>, ApiError> {
    let list = state.holidays.list().await?;
    Ok(Json(
        list.into_iter()
            .map(|c| CalendarDto {
                code: c.code,
                name: c.name,
            })
            .collect(),
    ))
}

async fn get_rules(
    State(state): State<Arc<AppState>>,
    _auth: AuthUserId,
    Path(code): Path<String>,
) -> Result<Json<Vec<RuleDto>>, ApiError> {
    let rules = state.holidays.rules(&code).await?;
    Ok(Json(rules.into_iter().map(RuleDto::from).collect()))
}

async fn preview_saved(
    State(state): State<Arc<AppState>>,
    _auth: AuthUserId,
    Path(code): Path<String>,
    Query(q): Query<YearQuery>,
) -> Result<Json<Vec<HolidayDto>>, ApiError> {
    let rules = state.holidays.rules(&code).await?;
    Ok(Json(holidays_dto(HolidayService::preview(&rules, q.year)?)))
}

async fn create_calendar(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateRequest>,
) -> Result<StatusCode, ApiError> {
    state.holidays.create(&body.code, &body.name).await?;
    Ok(StatusCode::CREATED)
}

async fn rename_calendar(
    State(state): State<Arc<AppState>>,
    Path(code): Path<String>,
    Json(body): Json<RenameRequest>,
) -> Result<StatusCode, ApiError> {
    state.holidays.rename(&code, &body.name).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_calendar(
    State(state): State<Arc<AppState>>,
    Path(code): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.holidays.delete(&code).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn replace_rules(
    State(state): State<Arc<AppState>>,
    Path(code): Path<String>,
    Json(body): Json<RulesRequest>,
) -> Result<StatusCode, ApiError> {
    let parts: Vec<RuleParts> = body.rules.into_iter().map(RuleParts::from).collect();
    state.holidays.replace_rules(&code, &parts).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Preview unsaved edits without writing anything.
async fn preview_rules(
    Json(body): Json<PreviewRequest>,
) -> Result<Json<Vec<HolidayDto>>, ApiError> {
    let parts: Vec<RuleParts> = body.rules.into_iter().map(RuleParts::from).collect();
    Ok(Json(holidays_dto(HolidayService::preview(
        &parts, body.year,
    )?)))
}
