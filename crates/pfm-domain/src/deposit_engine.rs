//! Pure deposit-interest engine: computes day-by-day interest on the effective
//! balance of a deposit account, applies the 26 % withholding, and returns the
//! gross / withholding / net amounts in cents.
//!
//! For each day of the period the gross daily interest is
//! `balance_cents × rate_bps / 10000 / 365`, where `rate_bps` is the gross
//! annual rate in basis points (e.g. 325 = 3.25 %). The total gross is the sum
//! of all daily interests, rounded half-even to the nearest cent. The
//! withholding is 26 % of the rounded gross (rounded half-even); the net is
//! `gross − withholding`, so `gross = withholding + net` holds exactly.
//!
//! Rounding: round-half-even (banker's rounding), consistent with
//! [`super::recurrence_engine::round_half_even`] and
//! [`super::fee_engine::pct_of`].
//!
//! This module is intentionally free of I/O, serialization, or framework
//! dependencies. The caller (the service layer) builds the per-day balance
//! series from the ledger and passes it here; the engine performs only the
//! arithmetic.

use chrono::NaiveDate;

/// A single day's effective (closing) balance on the deposit account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DailyBalance {
    /// The date of this day.
    pub date: NaiveDate,
    /// The closing balance at the end of this day, in cents.
    pub balance_cents: i64,
}

/// The result of a deposit-interest computation over a period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterestResult {
    /// Total gross interest accrued over the period, in cents (rounded
    /// half-even).
    pub gross_cents: i64,
    /// The 26 % withholding applied to the gross, in cents (rounded half-even).
    pub withholding_cents: i64,
    /// The net amount credited to the linked current account, in cents.
    /// Equals `gross_cents − withholding_cents`.
    pub net_cents: i64,
}

/// Compute deposit interest day by day over the effective balances.
///
/// `balances` is the ordered series of per-day closing balances for the period.
/// `rate_bps` is the gross annual rate in basis points (e.g. 325 = 3.25 %).
///
/// The total gross is `sum(balance_cents × rate_bps) / 3650000`, rounded
/// half-even. The withholding is `gross × 26 / 100`, rounded half-even. The net
/// is `gross − withholding`.
///
/// Returns all-zero when `balances` is empty or `rate_bps <= 0`.
pub fn compute_interest(balances: &[DailyBalance], rate_bps: i64) -> InterestResult {
    if balances.is_empty() || rate_bps <= 0 {
        return InterestResult {
            gross_cents: 0,
            withholding_cents: 0,
            net_cents: 0,
        };
    }

    // Accumulate balance × rate_bps in i128 so a large balance × rate product
    // cannot overflow i64. The denominator 10000 × 365 = 3 650 000 folds both
    // the basis-point scaling and the 365-day year into a single division.
    let total_num: i128 = balances
        .iter()
        .map(|b| b.balance_cents as i128 * rate_bps as i128)
        .sum();

    const DENOM: i128 = 3_650_000;
    let gross_cents = round_half_even_i128(total_num, DENOM);

    let withholding_cents = round_half_even_i128(gross_cents as i128 * 26, 100);
    let net_cents = gross_cents - withholding_cents;

    InterestResult {
        gross_cents,
        withholding_cents,
        net_cents,
    }
}

/// `numerator / denominator` rounded half-even (banker's rounding), computed in
/// `i128` to avoid overflow, then clamped to `i64`. `denominator` must be `> 0`.
fn round_half_even_i128(numerator: i128, denominator: i128) -> i64 {
    debug_assert!(denominator != 0);
    let q = numerator / denominator;
    let r = numerator % denominator;
    let twice_abs_r = 2 * r.unsigned_abs();
    let abs_den = denominator.unsigned_abs();

    let result = if twice_abs_r < abs_den {
        q
    } else if twice_abs_r > abs_den {
        if numerator >= 0 {
            q + 1
        } else {
            q - 1
        }
    } else if q % 2 == 0 {
        q
    } else if numerator >= 0 {
        q + 1
    } else {
        q - 1
    };

    result.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    /// Build a `DailyBalance` series where the balance is constant at
    /// `balance` for `days` consecutive days starting from `start`.
    fn constant_series(start: NaiveDate, days: u32, balance: i64) -> Vec<DailyBalance> {
        (0..days as i64)
            .map(|i| DailyBalance {
                date: start + Duration::days(i),
                balance_cents: balance,
            })
            .collect()
    }

    // ── zero / empty guards ─────────────────────────────────────────────────

    #[test]
    fn empty_series_returns_zero() {
        let r = compute_interest(&[], 325);
        assert_eq!(
            r,
            InterestResult {
                gross_cents: 0,
                withholding_cents: 0,
                net_cents: 0
            }
        );
    }

    #[test]
    fn zero_rate_returns_zero() {
        let s = constant_series(d("2024-01-01"), 30, 1_000_000);
        let r = compute_interest(&s, 0);
        assert_eq!(
            r,
            InterestResult {
                gross_cents: 0,
                withholding_cents: 0,
                net_cents: 0
            }
        );
    }

    #[test]
    fn negative_rate_returns_zero() {
        let s = constant_series(d("2024-01-01"), 30, 1_000_000);
        let r = compute_interest(&s, -100);
        assert_eq!(
            r,
            InterestResult {
                gross_cents: 0,
                withholding_cents: 0,
                net_cents: 0
            }
        );
    }

    // ── constant balance: simple sanity ─────────────────────────────────────

    /// A constant 10 000.00 EUR balance at 3.25 % for 365 days.
    ///
    /// Exact gross = 1 000 000 × 325 / 3 650 000 × 365 = 1 000 000 × 325 / 10000
    ///              = 32 500 cents = 325.00 EUR.
    /// Withholding = 32500 × 26 / 100 = 8450 cents.
    /// Net         = 32500 − 8450 = 24050 cents.
    #[test]
    fn constant_balance_full_year() {
        let s = constant_series(d("2024-01-01"), 366, 1_000_000);
        // 2024 is a leap year: 366 days.
        // Gross = 1000000 * 325 * 366 / 3650000 = 32627.39... -> 32627
        let r = compute_interest(&s, 325);
        // 1000000 * 325 = 325_000_000 per day
        // 325_000_000 * 366 = 118_950_000_000
        // 118_950_000_000 / 3_650_000 = 32589.04... -> 32589
        assert_eq!(r.gross_cents, 32_589);
        // 32589 * 26 / 100 = 8473.14 -> 8473
        assert_eq!(r.withholding_cents, 8_473);
        assert_eq!(r.net_cents, 32_589 - 8_473);
    }

    /// A constant 100 000.00 EUR balance at 3.00 % for exactly 365 days.
    ///
    /// Gross = 10 000 000 × 300 / 10000 = 300 000 cents = 3000.00 EUR.
    /// Withholding = 300000 × 26 / 100 = 78000.
    /// Net = 222000.
    #[test]
    fn constant_balance_365_days_round_number() {
        let s = constant_series(d("2023-01-01"), 365, 10_000_000);
        // 2023 is not a leap year: 365 days.
        let r = compute_interest(&s, 300);
        // 10_000_000 * 300 = 3_000_000_000 per day
        // 3_000_000_000 * 365 = 1_095_000_000_000
        // 1_095_000_000_000 / 3_650_000 = 300_000 exactly
        assert_eq!(r.gross_cents, 300_000);
        assert_eq!(r.withholding_cents, 78_000);
        assert_eq!(r.net_cents, 222_000);
    }

    // ── gate: mid-year deposit → day-by-day ≠ simple annual ─────────────────

    /// Gate 1: a deposit made mid-year must produce a day-by-day result that
    /// differs from (and is more correct than) a simple annual formula applied
    /// to the final balance.
    ///
    /// Scenario: an account opened 2024-01-01 with 0. On 2024-07-01 a
    /// deposit of 12 155.06 EUR (1 215 506 cents) is made. Rate 3.25 %.
    /// Period: 2024-01-01 .. 2024-12-31 (366 days, leap year).
    ///
    /// Day-by-day:
    ///   Jan 1 – Jun 30  (182 days): balance = 0        → interest = 0
    ///   Jul 1 – Dec 31  (184 days): balance = 1 215 506
    ///   gross = 1 215 506 × 325 × 184 / 3 650 000
    ///         = 72 687 258 800 / 3 650 000
    ///         = 19 914.31… → 19 914 cents
    ///
    /// Simple annual (incorrect):
    ///   1 215 506 × 325 / 10000 = 39 503.945 → 39 504 cents
    ///
    /// The two values must differ, proving the day-by-day method is used.
    #[test]
    fn mid_year_deposit_differs_from_simple_annual() {
        let start = d("2024-01-01");
        let deposit_date = d("2024-07-01");
        let end = d("2024-12-31");
        let deposit_cents = 1_215_506i64;
        let rate_bps = 325i64;

        // Build the daily balance series.
        let mut series: Vec<DailyBalance> = Vec::new();
        let mut day = start;
        while day <= end {
            let balance = if day >= deposit_date {
                deposit_cents
            } else {
                0
            };
            series.push(DailyBalance {
                date: day,
                balance_cents: balance,
            });
            day += Duration::days(1);
        }

        let result = compute_interest(&series, rate_bps);

        // Day-by-day gross: only the 184 days from Jul 1 to Dec 31 contribute.
        let contributing_days = 184i128;
        let expected_gross = round_half_even_i128(
            deposit_cents as i128 * rate_bps as i128 * contributing_days,
            3_650_000,
        );
        assert_eq!(result.gross_cents, expected_gross);
        assert_eq!(result.gross_cents, 19_914);

        // Simple annual formula (the "wrong" way): applies the full-year rate
        // to the final balance as if it had been present the whole year.
        let simple_annual = round_half_even_i128(deposit_cents as i128 * rate_bps as i128, 10_000);
        assert_eq!(simple_annual, 39_504);

        // The day-by-day result must differ from the simple annual result.
        assert_ne!(
            result.gross_cents, simple_annual,
            "day-by-day calculation produced the same result as the simple \
             annual formula — the mid-year deposit is not being accounted for \
             correctly"
        );

        // Withholding and net are derived from the gross.
        assert_eq!(
            result.withholding_cents,
            round_half_even_i128(result.gross_cents as i128 * 26, 100)
        );
        assert_eq!(
            result.net_cents,
            result.gross_cents - result.withholding_cents
        );
    }

    /// Gate 1 (additional probe): two deposits at different times. The
    /// day-by-day result must reflect both deposits' actual presence on each
    /// day, which a single final-balance formula cannot capture.
    #[test]
    fn two_mid_year_deposits_day_by_day() {
        let start = d("2024-01-01");
        let end = d("2024-12-31");
        let rate_bps = 325i64;

        // Deposit A: 100 000 cents on Jan 1 (present all year).
        // Deposit B: 200 000 cents on Jul 1 (present Jul 1 – Dec 31).
        let mut series: Vec<DailyBalance> = Vec::new();
        let mut day = start;
        while day <= end {
            let balance = 100_000 + if day >= d("2024-07-01") { 200_000 } else { 0 };
            series.push(DailyBalance {
                date: day,
                balance_cents: balance,
            });
            day += Duration::days(1);
        }

        let result = compute_interest(&series, rate_bps);

        // Hand-computed:
        // Jan 1 – Jun 30:  182 days × 100 000 × 325 / 3 650 000 = 1 610.95… → per-day sum
        // Jul 1 – Dec 31:  184 days × 300 000 × 325 / 3 650 000
        // Total = (182 × 100_000 + 184 × 300_000) × 325 / 3_650_000
        //       = (18_200_000 + 55_200_000) × 325 / 3_650_000
        //       = 73_400_000 × 325 / 3_650_000
        //       = 23_855_000_000 / 3_650_000
        //       = 6535.61… → 6536
        let expected = round_half_even_i128(73_400_000i128 * 325, 3_650_000);
        assert_eq!(result.gross_cents, expected);
        assert_eq!(result.gross_cents, 6_536);

        // A simple annual formula on the final balance (300 000) would give:
        // 300_000 × 325 / 10000 = 9750, which is wrong.
        let wrong = round_half_even_i128(300_000i128 * 325, 10_000);
        assert_ne!(result.gross_cents, wrong);
    }

    // ── gate: withholding 26 % with round-half-even ─────────────────────────

    /// Gate 2: the 26 % withholding is applied to the rounded gross and uses
    /// round-half-even, consistent with the rest of the system.
    #[test]
    fn withholding_26_percent_round_half_even() {
        // Construct a scenario where the gross rounds to a value whose 26 %
        // is a half-cent, exercising the banker's-rounding tie-break.
        //
        // gross = 501 cents → 501 × 26 / 100 = 130.26 → 130 (no tie).
        // gross = 500 cents → 500 × 26 / 100 = 130.00 → 130 (exact).
        // gross = 250 cents → 250 × 26 / 100 = 65.00  → 65  (exact).
        //
        // For a true half tie we need gross × 26 ≡ 50 (mod 100), i.e.
        // gross × 26 ends in …50. gross = 5 → 130/100 = 1.30 → 1.
        // gross = 15 → 390/100 = 3.90 → 4.
        // gross = 25 → 650/100 = 6.50 → tie: 6 is even → 6.
        // gross = 35 → 910/100 = 9.10 → 9.
        // gross = 45 → 1170/100 = 11.70 → 12.
        // gross = 55 → 1430/100 = 14.30 → 14.
        //
        // Let's use gross = 25: withholding = 650/100 = 6.5 → round to even = 6.
        assert_eq!(round_half_even_i128(25 * 26, 100), 6);
        // gross = 75: 75 × 26 = 1950/100 = 19.5 → round to even = 20.
        assert_eq!(round_half_even_i128(75 * 26, 100), 20);

        // Now verify through compute_interest with a crafted balance series.
        // We need a gross of exactly 25 cents.
        // balance × rate_bps × days / 3650000 = 25
        // With rate_bps = 10000 (100 %) and 1 day:
        // balance × 10000 / 3650000 = 25 → balance = 25 × 365 = 9125.
        let s = vec![DailyBalance {
            date: d("2024-01-01"),
            balance_cents: 9_125,
        }];
        let r = compute_interest(&s, 10_000);
        // 9125 × 10000 = 91_250_000 / 3_650_000 = 25.0 exactly.
        assert_eq!(r.gross_cents, 25);
        // 25 × 26 = 650 / 100 = 6.5 → round to even → 6.
        assert_eq!(r.withholding_cents, 6);
        assert_eq!(r.net_cents, 19);
    }

    /// Verify the withholding identity: gross = withholding + net, for several
    /// gross values including tie-breaking cases.
    #[test]
    fn gross_equals_withholding_plus_net() {
        let cases: i64 = 200;
        for gross in 0..=cases {
            let w = round_half_even_i128(gross as i128 * 26, 100);
            let n = gross - w;
            assert_eq!(w + n, gross, "identity violated for gross={gross}");
            assert!(w >= 0 && n >= 0, "negative component for gross={gross}");
        }
    }

    // ── rounding edge cases ─────────────────────────────────────────────────

    #[test]
    fn round_half_even_i128_cases() {
        // 7/2 = 3.5 → 4 (round to even)
        assert_eq!(round_half_even_i128(7, 2), 4);
        // 9/2 = 4.5 → 4 (round to even)
        assert_eq!(round_half_even_i128(9, 2), 4);
        // 8/2 = 4 → 4
        assert_eq!(round_half_even_i128(8, 2), 4);
        // 10/4 = 2.5 → 2 (round to even)
        assert_eq!(round_half_even_i128(10, 4), 2);
        // 14/4 = 3.5 → 4 (round to even)
        assert_eq!(round_half_even_i128(14, 4), 4);
        // -7/2 = -3.5 → -4 (round to even)
        assert_eq!(round_half_even_i128(-7, 2), -4);
        // -9/2 = -4.5 → -4 (round to even)
        assert_eq!(round_half_even_i128(-9, 2), -4);
        // 1/3 = 0.33 → 0
        assert_eq!(round_half_even_i128(1, 3), 0);
        // 2/3 = 0.67 → 1
        assert_eq!(round_half_even_i128(2, 3), 1);
        // 0/anything = 0
        assert_eq!(round_half_even_i128(0, 100), 0);
    }

    /// A single day with a known balance and rate: verify the exact result.
    #[test]
    fn single_day_exact() {
        // balance = 1_000_000 cents (10 000.00), rate = 365 bps (3.65 %).
        // Daily interest = 1_000_000 × 365 / 3_650_000 = 100 cents exactly.
        let s = vec![DailyBalance {
            date: d("2024-01-01"),
            balance_cents: 1_000_000,
        }];
        let r = compute_interest(&s, 365);
        assert_eq!(r.gross_cents, 100);
        assert_eq!(r.withholding_cents, 26);
        assert_eq!(r.net_cents, 74);
    }

    /// Two days with the same balance: the gross is exactly double the single-day
    /// value.
    #[test]
    fn two_days_double_single_day() {
        let s = vec![
            DailyBalance {
                date: d("2024-01-01"),
                balance_cents: 1_000_000,
            },
            DailyBalance {
                date: d("2024-01-02"),
                balance_cents: 1_000_000,
            },
        ];
        let r = compute_interest(&s, 365);
        // 2 × 100 = 200.
        assert_eq!(r.gross_cents, 200);
        assert_eq!(r.withholding_cents, 52);
        assert_eq!(r.net_cents, 148);
    }

    /// Zero balance for all days: no interest.
    #[test]
    fn zero_balance_no_interest() {
        let s = constant_series(d("2024-01-01"), 30, 0);
        let r = compute_interest(&s, 325);
        assert_eq!(
            r,
            InterestResult {
                gross_cents: 0,
                withholding_cents: 0,
                net_cents: 0
            }
        );
    }

    /// Negative balance: the engine computes the arithmetic honestly (negative
    /// interest). The service layer is responsible for validating that a
    /// deposit account does not go negative.
    #[test]
    fn negative_balance_produces_negative_interest() {
        let s = vec![DailyBalance {
            date: d("2024-01-01"),
            balance_cents: -1_000_000,
        }];
        let r = compute_interest(&s, 365);
        assert_eq!(r.gross_cents, -100);
    }
}
