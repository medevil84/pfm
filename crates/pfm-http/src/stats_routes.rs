//! Stats route handlers: `GET /stats` (KPIs, expenses by category, balance
//! trend with future projection, budget progress).

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::NaiveDate;

use pfm_repo::RegisterFilters;
use pfm_service::StatsError;

use crate::dto::{
    BalancePointDto, BudgetDto, CategorySpendDto, KpiDto, StatsQueryParams, StatsResponse,
};
use crate::error::ApiError;
use crate::session::AuthUserId;
use crate::state::AppState;

/// Build the stats routes router.
pub fn stats_routes() -> Router<Arc<AppState>> {
    Router::new().route("/stats", get(get_stats))
}

/// The filter knobs that apply to the expenses-by-category breakdown. The
/// KPIs, balance trend and budget always use the full (unfiltered) ledger.
fn to_filters(params: &StatsQueryParams) -> RegisterFilters {
    RegisterFilters {
        category_id: params.cat.clone(),
        tx_type: None,
        min_cents: None,
        max_cents: None,
        text: None,
        vacct_included: Some(params.vacct_included.0.clone()),
    }
}

fn parse_date(value: &str) -> Result<NaiveDate, ApiError> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| ApiError::bad_request("errors.invalid_date"))
}

/// `GET /stats?account&from&to&period&cat&vacct_included`
async fn get_stats(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Query(params): Query<StatsQueryParams>,
) -> Result<Json<StatsResponse>, ApiError> {
    if params.from.is_some() != params.to.is_some() {
        return Err(ApiError::bad_request("errors.date_range_required"));
    }
    if let Some(p) = &params.period {
        if p != "month" && p != "year" {
            return Err(ApiError::bad_request("errors.period_invalid"));
        }
    }

    let from = params.from.as_deref().map(parse_date).transpose()?;
    let to = params.to.as_deref().map(parse_date).transpose()?;

    let filters = to_filters(&params);
    let result = state
        .stats
        .stats(
            &user_id,
            &params.account,
            from,
            to,
            params.period.as_deref(),
            &filters,
        )
        .await?;

    Ok(Json(StatsResponse {
        account_id: params.account,
        from: result.from.format("%Y-%m-%d").to_string(),
        to: result.to.format("%Y-%m-%d").to_string(),
        kpi: KpiDto::from(&result.kpi),
        expenses_by_category: result
            .expenses_by_category
            .iter()
            .map(CategorySpendDto::from)
            .collect(),
        balance_trend: result
            .balance_trend
            .iter()
            .map(BalancePointDto::from)
            .collect(),
        budget: result.budget.iter().map(BudgetDto::from).collect(),
    }))
}

impl From<StatsError> for ApiError {
    fn from(e: StatsError) -> Self {
        match e {
            StatsError::AccountNotFound => Self::not_found("errors.account_not_found"),
            StatsError::Forbidden => Self::forbidden(),
            StatsError::Validation(msg) => Self::with_args(
                axum::http::StatusCode::BAD_REQUEST,
                "errors.validation",
                msg,
            ),
            StatsError::Internal(msg) => Self::internal(Some(msg)),
        }
    }
}
