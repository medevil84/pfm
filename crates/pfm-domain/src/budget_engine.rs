//! Pure budget engine: computes per-category budget progress and the
//! same-length previous-period date range.
//!
//! This module is intentionally free of I/O, serialization, or framework
//! dependencies. The caller (the service layer) supplies the category's
//! budget, the amount actually spent in the requested period, and the amount
//! spent in the previous period; this module only derives the progress
//! percentage and the previous period's bounds.

use chrono::{Duration, NaiveDate};

/// The previous period's date range for a given period.
///
/// The previous period has the same number of days as `[from, to]` and ends
/// on the day immediately before `from`. This handles months of different
/// length (28/29/30/31 days) correctly: a 31-day period in March is compared
/// against the preceding 31 days of February/March, not against "last month".
pub fn prev_period_range(from: NaiveDate, to: NaiveDate) -> (NaiveDate, NaiveDate) {
    let days = (to - from).num_days() + 1;
    let prev_to = from - Duration::days(1);
    let prev_from = prev_to - Duration::days(days - 1);
    (prev_from, prev_to)
}

/// Per-category budget progress.
#[derive(Debug, Clone, PartialEq)]
pub struct BudgetProgress {
    /// The category id.
    pub category_id: String,
    /// The category display name.
    pub category_name: String,
    /// The monthly budget in cents.
    pub budget_cents: i64,
    /// The amount spent (positive magnitude) in the requested period.
    pub spent_cents: i64,
    /// The amount spent (positive magnitude) in the previous same-length period.
    pub prev_period_cents: i64,
    /// Percentage of the budget consumed by `spent_cents` (0..=100+), rounded
    /// to one decimal place. `0.0` when `budget_cents <= 0`.
    pub pct: f64,
}

/// Compute the budget progress for a single category.
///
/// `spent_cents` and `prev_period_cents` are positive magnitudes (the absolute
/// value of the outflow). `budget_cents` is the configured monthly budget.
pub fn budget_progress(
    category_id: &str,
    category_name: &str,
    budget_cents: i64,
    spent_cents: i64,
    prev_period_cents: i64,
) -> BudgetProgress {
    let pct = if budget_cents > 0 {
        let raw = (spent_cents as f64 / budget_cents as f64) * 100.0;
        (raw * 10.0).round() / 10.0
    } else {
        0.0
    };

    BudgetProgress {
        category_id: category_id.to_string(),
        category_name: category_name.to_string(),
        budget_cents,
        spent_cents,
        prev_period_cents,
        pct,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn prev_period_same_length() {
        let (pf, pt) = prev_period_range(d("2024-01-01"), d("2024-01-31"));
        assert_eq!(pf, d("2023-12-01"));
        assert_eq!(pt, d("2023-12-31"));
    }

    #[test]
    fn prev_period_feb_leap() {
        let (pf, pt) = prev_period_range(d("2024-02-01"), d("2024-02-29"));
        assert_eq!(pf, d("2024-01-03"));
        assert_eq!(pt, d("2024-01-31"));
    }

    #[test]
    fn prev_period_feb_nonleap() {
        let (pf, pt) = prev_period_range(d("2023-02-01"), d("2023-02-28"));
        assert_eq!(pf, d("2023-01-04"));
        assert_eq!(pt, d("2023-01-31"));
    }

    #[test]
    fn prev_period_april_follows_31_day_march() {
        let (pf, pt) = prev_period_range(d("2024-04-01"), d("2024-04-30"));
        assert_eq!(pf, d("2024-03-02"));
        assert_eq!(pt, d("2024-03-31"));
    }

    #[test]
    fn prev_period_single_day() {
        let (pf, pt) = prev_period_range(d("2024-06-15"), d("2024-06-15"));
        assert_eq!(pf, d("2024-06-14"));
        assert_eq!(pt, d("2024-06-14"));
    }

    #[test]
    fn prev_period_cross_year() {
        let (pf, pt) = prev_period_range(d("2024-01-01"), d("2024-01-31"));
        assert_eq!(pf.year(), 2023);
        assert_eq!(pt.year(), 2023);
    }

    #[test]
    fn budget_progress_normal() {
        let p = budget_progress("cat1", "Food", 100_000, 25_000, 30_000);
        assert_eq!(p.budget_cents, 100_000);
        assert_eq!(p.spent_cents, 25_000);
        assert_eq!(p.prev_period_cents, 30_000);
        assert!((p.pct - 25.0).abs() < f64::EPSILON);
    }

    #[test]
    fn budget_progress_over_budget() {
        let p = budget_progress("cat1", "Food", 50_000, 75_000, 60_000);
        assert!((p.pct - 150.0).abs() < f64::EPSILON);
    }

    #[test]
    fn budget_progress_zero_budget() {
        let p = budget_progress("cat1", "Food", 0, 10_000, 5_000);
        assert_eq!(p.pct, 0.0);
    }

    #[test]
    fn budget_progress_rounding() {
        let p = budget_progress("cat1", "Food", 3_000, 1_000, 0);
        // 1000/3000 = 33.333...% -> 33.3
        assert!((p.pct - 33.3).abs() < f64::EPSILON);
    }
}
