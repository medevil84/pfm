//! Pure fee engine: evaluates account "spend rules" (canoni, imposte di bollo,
//! commissioni) and produces the fee rows to materialize.
//!
//! This module is intentionally free of I/O, serialization, or framework
//! dependencies. It receives a rule plus the *already computed* magnitudes of
//! each competence period (average balance, closing balance, total out, total
//! in, transaction count) and applies only the rule logic: cadence, charge
//! date, condition, amount, annual distribution and annual cap.
//!
//! The engine does **not** compute those magnitudes itself, nor does it persist
//! anything: the caller (the service layer) supplies per-period metrics via a
//! closure and stores the returned rows. Rows produced by the engine are not
//! counted in the caller's magnitude computation (the caller excludes engine
//! rows from the balances it feeds back in).

use chrono::{Datelike, Duration, NaiveDate};

use crate::calendar::HolidayCalendar;

/// How the fee amount for a period is computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalcType {
    /// A fixed amount: `value` is the amount in cents.
    Fixed,
    /// A percentage of the period's average balance: `value` is in basis points
    /// (e.g. 20 = 0.20%).
    PctAvgBalance,
    /// A percentage of the period's closing balance: `value` is in basis points.
    PctEndBalance,
}

impl CalcType {
    /// Stable string form used on the wire and in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            CalcType::Fixed => "fixed",
            CalcType::PctAvgBalance => "pct_avg_balance",
            CalcType::PctEndBalance => "pct_end_balance",
        }
    }

    /// Parse a calculation type from its string form. `None` for unknown values.
    pub fn parse(s: &str) -> Option<CalcType> {
        Some(match s {
            "fixed" => CalcType::Fixed,
            "pct_avg_balance" => CalcType::PctAvgBalance,
            "pct_end_balance" => CalcType::PctEndBalance,
            _ => return None,
        })
    }
}

impl std::fmt::Display for CalcType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How often the fee is charged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cadence {
    /// Once per month.
    Monthly,
    /// Once every two months.
    Bimonthly,
    /// Once every three months.
    Quarterly,
    /// Once every six months.
    Semiannual,
    /// Once per year.
    Annual,
}

impl Cadence {
    /// Number of months spanned by one competence period of this cadence.
    pub fn months_per_period(self) -> u32 {
        match self {
            Cadence::Monthly => 1,
            Cadence::Bimonthly => 2,
            Cadence::Quarterly => 3,
            Cadence::Semiannual => 6,
            Cadence::Annual => 12,
        }
    }

    /// Stable string form used on the wire and in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            Cadence::Monthly => "monthly",
            Cadence::Bimonthly => "bimonthly",
            Cadence::Quarterly => "quarterly",
            Cadence::Semiannual => "semiannual",
            Cadence::Annual => "annual",
        }
    }

    /// Parse a cadence from its string form. `None` for unknown values.
    pub fn parse(s: &str) -> Option<Cadence> {
        Some(match s {
            "monthly" => Cadence::Monthly,
            "bimonthly" => Cadence::Bimonthly,
            "quarterly" => Cadence::Quarterly,
            "semiannual" => Cadence::Semiannual,
            "annual" => Cadence::Annual,
            _ => return None,
        })
    }
}

impl std::fmt::Display for Cadence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The magnitude a fee condition is evaluated against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CondBase {
    /// No condition: the fee always applies.
    Always,
    /// The period's average balance (cents).
    AvgBalance,
    /// The period's closing balance (cents).
    EndBalance,
    /// The period's total outflows (cents, positive magnitude).
    TotalOut,
    /// The period's total inflows (cents, positive magnitude).
    TotalIn,
    /// The period's number of transactions (count).
    TxCount,
}

impl CondBase {
    /// Stable string form used on the wire and in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            CondBase::Always => "always",
            CondBase::AvgBalance => "avg_balance",
            CondBase::EndBalance => "end_balance",
            CondBase::TotalOut => "total_out",
            CondBase::TotalIn => "total_in",
            CondBase::TxCount => "tx_count",
        }
    }

    /// Parse a condition base from its string form. `None` for unknown values.
    pub fn parse(s: &str) -> Option<CondBase> {
        Some(match s {
            "always" => CondBase::Always,
            "avg_balance" => CondBase::AvgBalance,
            "end_balance" => CondBase::EndBalance,
            "total_out" => CondBase::TotalOut,
            "total_in" => CondBase::TotalIn,
            "tx_count" => CondBase::TxCount,
            _ => return None,
        })
    }
}

impl std::fmt::Display for CondBase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The comparison operator of a fee condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CondOp {
    /// Strictly greater than.
    Gt,
    /// Greater than or equal to.
    Ge,
    /// Strictly less than.
    Lt,
    /// Less than or equal to.
    Le,
}

impl CondOp {
    /// Stable string form used on the wire and in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            CondOp::Gt => ">",
            CondOp::Ge => ">=",
            CondOp::Lt => "<",
            CondOp::Le => "<=",
        }
    }

    /// Parse a comparison operator from its string form. `None` for unknown values.
    pub fn parse(s: &str) -> Option<CondOp> {
        Some(match s {
            ">" => CondOp::Gt,
            ">=" => CondOp::Ge,
            "<" => CondOp::Lt,
            "<=" => CondOp::Le,
            _ => return None,
        })
    }
}

impl std::fmt::Display for CondOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A fee rule to evaluate (the input to [`generate_fees`]).
///
/// `value` is interpreted by [`CalcType`]: cents for [`CalcType::Fixed`], basis
/// points for the percentage types. When `is_annual` is set, `value` is the
/// *annual* amount to be distributed over the year's charged deadlines;
/// otherwise it is the per-period amount.
#[derive(Debug, Clone, Copy)]
pub struct FeeRule {
    /// How the amount is computed.
    pub calc_type: CalcType,
    /// Amount: cents (fixed) or basis points (percentage).
    pub value: i64,
    /// Whether `value` is an annual amount to distribute over the year's deadlines.
    pub is_annual: bool,
    /// Whether a fixed annual amount accrues per day (see [`accrues_daily`]):
    /// pro-rata on the period's actual days, debited in arrears.
    pub prorata_days: bool,
    /// Optional cap on the total charged within a calendar year (cents).
    pub annual_cap_cents: Option<i64>,
    /// How often the fee is charged.
    pub cadence: Cadence,
    /// Day of the charge month: `0` means the last day of that month.
    pub charge_day: u32,
    /// Charge month (1-12) for the annual cadence. `None` defaults to December.
    pub charge_month: Option<u32>,
    /// What the condition is evaluated against.
    pub cond_base: CondBase,
    /// Comparison operator (ignored when [`CondBase::Always`]).
    pub cond_op: Option<CondOp>,
    /// Threshold: cents for balance/flow bases, a count for [`CondBase::TxCount`].
    pub cond_threshold: Option<i64>,
}

/// A competence period with its charge date, as produced by the cadence.
#[derive(Debug, Clone, Copy)]
pub struct FeePeriod {
    /// First day of the competence period.
    pub from: NaiveDate,
    /// Last day of the competence period.
    pub to: NaiveDate,
    /// The date on which the fee is charged (within the period's last month).
    pub date: NaiveDate,
}

/// The already-computed magnitudes of a competence period, supplied by the
/// caller. All amounts are in cents. `tx_count` is a plain count.
#[derive(Debug, Clone, Copy, Default)]
pub struct PeriodMetrics {
    /// Average daily balance over the period (cents).
    pub avg_balance_cents: i64,
    /// Closing balance at the end of the period (cents).
    pub end_balance_cents: i64,
    /// Total outflows over the period (positive magnitude, cents).
    pub total_out_cents: i64,
    /// Total inflows over the period (positive magnitude, cents).
    pub total_in_cents: i64,
    /// Number of transactions in the period.
    pub tx_count: i64,
}

/// A fee row to materialize. `amount_cents` is signed: fees are outflows, so it
/// is negative (consistent with the transaction sign convention).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeeRow {
    /// The charge date.
    pub date: NaiveDate,
    /// The fee amount in cents (negative for the charge).
    pub amount_cents: i64,
}

/// Evaluate `rule` over the horizon `[from, to]` and return the fee rows to
/// materialize, sorted by charge date.
///
/// `metrics` is called once per generated deadline with that deadline's
/// competence [`FeePeriod`]; it must return the already-computed magnitudes for
/// that period (the engine never computes them, and the caller is responsible
/// for excluding engine-produced rows from them).
///
/// For each deadline whose condition is satisfied a row is emitted. When
/// `is_annual` is set, the annual value is distributed over the charged
/// deadlines of the calendar year (the rounding remainder is placed on the last
/// charged deadline so the year's total is exact). When `annual_cap_cents` is
/// set, the cumulative charge within each calendar year is truncated so it
/// never exceeds the cap.
pub fn generate_fees<F>(rule: &FeeRule, from: NaiveDate, to: NaiveDate, metrics: F) -> Vec<FeeRow>
where
    F: Fn(&FeePeriod) -> PeriodMetrics,
{
    generate_fees_in(rule, from, to, &HolidayCalendar::weekends_only(), metrics)
}

/// Like [`generate_fees`], but rules that are debited in arrears (see
/// [`FeeRule::prorata_days`]) are dated on the first business day of
/// `calendar` after the period ends.
pub fn generate_fees_in<F>(
    rule: &FeeRule,
    from: NaiveDate,
    to: NaiveDate,
    calendar: &HolidayCalendar,
    metrics: F,
) -> Vec<FeeRow>
where
    F: Fn(&FeePeriod) -> PeriodMetrics,
{
    if from > to {
        return Vec::new();
    }

    let mut periods = charge_periods(rule, from, to, calendar);
    periods.sort_by_key(|p| p.date);

    let mut rows: Vec<FeeRow> = Vec::new();

    // Group deadlines by the calendar year of their competence period (a
    // period charged in arrears, e.g. Q4 charged in January, still belongs to
    // the year it accrued in). The groups are already in year order because
    // `periods` is sorted by charge date.
    let mut idx = 0usize;
    while idx < periods.len() {
        let year = periods[idx].to.year();
        let mut end = idx;
        while end < periods.len() && periods[end].to.year() == year {
            end += 1;
        }
        rows.extend(year_rows(rule, &periods, idx..end, &metrics));
        idx = end;
    }

    rows
}

/// Compute the fee rows for the deadlines in `range` (all sharing a calendar year).
fn year_rows<F>(
    rule: &FeeRule,
    periods: &[FeePeriod],
    range: std::ops::Range<usize>,
    metrics: &F,
) -> Vec<FeeRow>
where
    F: Fn(&FeePeriod) -> PeriodMetrics,
{
    // Deadlines that pass the condition and would produce a positive charge.
    let mut active: Vec<(usize, i64)> = Vec::new();
    for i in range {
        let m = metrics(&periods[i]);
        if !condition_passes(rule, &m) {
            continue;
        }
        let raw = raw_magnitude(rule, &m);
        if raw > 0 {
            active.push((i, raw));
        }
    }
    if active.is_empty() {
        return Vec::new();
    }

    // Per-deadline charge magnitudes.
    let n = active.len();
    let mags: Vec<i64> = if rule.is_annual {
        match rule.calc_type {
            CalcType::Fixed if accrues_daily(rule) => prorata_by_days(rule.value, periods, &active),
            CalcType::Fixed => split_exact(rule.value, n),
            _ => active
                .iter()
                .map(|(_, raw)| div_round(*raw, n as i64))
                .collect(),
        }
    } else {
        active.iter().map(|(_, raw)| *raw).collect()
    };

    // Apply the annual (calendar-year) cap, truncating the running total.
    let mut rows = Vec::new();
    let mut running: i64 = 0;
    for (k, &(i, _)) in active.iter().enumerate() {
        let mut mag = mags[k];
        if let Some(cap) = rule.annual_cap_cents {
            let room = cap.saturating_sub(running);
            if room <= 0 {
                continue;
            }
            mag = mag.min(room);
        }
        if mag <= 0 {
            continue;
        }
        running += mag;
        rows.push(FeeRow {
            date: periods[i].date,
            amount_cents: -mag,
        });
    }
    rows
}

/// Whether the rule's fixed annual amount accrues per day (stamp duty style):
/// when `prorata_days` is set, an annual amount on a sub-annual cadence is charged, for each period, in
/// proportion to the period's actual days over the days of its calendar year,
/// and is debited in arrears on the first working day after the period ends.
fn accrues_daily(rule: &FeeRule) -> bool {
    rule.prorata_days
        && rule.is_annual
        && rule.calc_type == CalcType::Fixed
        && rule.cadence != Cadence::Annual
}

/// Split the annual `value` over the `active` deadlines by their actual days.
/// Each deadline gets `value * cumulative_days / year_days` (rounded half up)
/// minus what the previous deadlines already got, so a fully charged year sums
/// exactly to `value` and the rounding never drifts.
fn prorata_by_days(value: i64, periods: &[FeePeriod], active: &[(usize, i64)]) -> Vec<i64> {
    let mut out = Vec::with_capacity(active.len());
    let mut cum_days: i64 = 0;
    let mut prev: i64 = 0;
    for &(i, _) in active {
        let p = &periods[i];
        let year = p.to.year();
        let year_days = days_in_year(year);
        cum_days += (p.to - p.from).num_days() + 1;
        let cum = div_round(
            ((value as i128) * (cum_days as i128)).clamp(0, i64::MAX as i128) as i64,
            year_days,
        );
        out.push((cum - prev).max(0));
        prev = cum;
    }
    out
}

fn days_in_year(year: i32) -> i64 {
    let first = NaiveDate::from_ymd_opt(year, 1, 1).expect("valid year");
    let next = NaiveDate::from_ymd_opt(year + 1, 1, 1).expect("valid year");
    (next - first).num_days()
}

/// Whether the rule's condition is satisfied for the given period metrics.
fn condition_passes(rule: &FeeRule, m: &PeriodMetrics) -> bool {
    let metric: i64 = match rule.cond_base {
        CondBase::Always => return true,
        CondBase::AvgBalance => m.avg_balance_cents,
        CondBase::EndBalance => m.end_balance_cents,
        CondBase::TotalOut => m.total_out_cents,
        CondBase::TotalIn => m.total_in_cents,
        CondBase::TxCount => m.tx_count,
    };
    let op = match rule.cond_op {
        Some(op) => op,
        None => return true,
    };
    let threshold = rule.cond_threshold.unwrap_or(0);
    match op {
        CondOp::Gt => metric > threshold,
        CondOp::Ge => metric >= threshold,
        CondOp::Lt => metric < threshold,
        CondOp::Le => metric <= threshold,
    }
}

/// The onere (fee) a rule contributes to a single **card settlement cycle**, in
/// cents (`>= 0`), or `0` when the rule does not apply to that cycle.
///
/// Card statements are monthly, so a rule fires on the cycle whose `closing_date`
/// falls in a month that ends one of the rule's competence periods — aligned to
/// the calendar exactly the way [`charge_periods`] aligns non-annual cadences
/// (monthly → every cycle; quarterly → Mar/Jun/Sep/Dec; annual → its
/// `charge_month`, December by default). The rule's own `charge_day` is ignored:
/// on a card the onere is always applied at the cycle close, not on the rule's
/// calendar day. The condition is evaluated on the cycle's own `metrics`
/// (supplied by the caller from the cycle's real movements). Annual amounts are
/// spread evenly over the year's periods; `annual_cap_cents` is not enforced
/// per-cycle.
///
/// This is the card-cycle counterpart to [`generate_fees`]: the fee engine stays
/// the single source of truth for how a rule's amount and condition are applied.
pub fn cycle_fee_amount(rule: &FeeRule, closing_date: NaiveDate, m: &PeriodMetrics) -> i64 {
    let passo = rule.cadence.months_per_period();
    let anchor0 = rule.charge_month.unwrap_or(12).clamp(1, 12) - 1;

    let fires = if rule.cadence == Cadence::Annual {
        closing_date.month0() == anchor0
    } else {
        closing_date.month0() % passo == anchor0 % passo
    };
    if !fires || !condition_passes(rule, m) {
        return 0;
    }

    let amount = if rule.is_annual {
        let periods_per_year = (12 / passo) as i64;
        match rule.calc_type {
            CalcType::Fixed => div_round(rule.value.max(0), periods_per_year),
            _ => raw_magnitude(rule, m),
        }
    } else {
        raw_magnitude(rule, m)
    };
    amount.max(0)
}

/// The per-period (non-annual) positive charge magnitude for the rule.
fn raw_magnitude(rule: &FeeRule, m: &PeriodMetrics) -> i64 {
    match rule.calc_type {
        CalcType::Fixed => rule.value.max(0),
        CalcType::PctAvgBalance => pct_of(m.avg_balance_cents.max(0), rule.value),
        CalcType::PctEndBalance => pct_of(m.end_balance_cents.max(0), rule.value),
    }
}

/// `base` in cents times `bps` basis points, over 10000, rounded half up.
fn pct_of(base: i64, bps: i64) -> i64 {
    if base <= 0 || bps <= 0 {
        return 0;
    }
    let num = (base as i128) * (bps as i128);
    let rounded = (2 * num + 10000) / 20000;
    rounded.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

/// `a / b` rounded half up, for `a >= 0` and `b > 0`.
fn div_round(a: i64, b: i64) -> i64 {
    if b <= 0 || a <= 0 {
        return 0;
    }
    let (a, b) = (a as i128, b as i128);
    ((2 * a + b) / (2 * b)) as i64
}

/// Split `total` into `n` parts that sum exactly to `total`, placing the
/// rounding remainder on the last part. Returns `n` zero parts if `total <= 0`.
fn split_exact(total: i64, n: usize) -> Vec<i64> {
    if n == 0 {
        return Vec::new();
    }
    if total <= 0 {
        return vec![0; n];
    }
    let base = total / n as i64;
    let mut parts = vec![base; n];
    let remainder = total - base * n as i64;
    *parts.last_mut().expect("n > 0") += remainder;
    parts
}

/// The competence periods (with charge dates) whose charge date falls in
/// `[from, to]`, sorted by charge date.
fn charge_periods(
    rule: &FeeRule,
    from: NaiveDate,
    to: NaiveDate,
    calendar: &HolidayCalendar,
) -> Vec<FeePeriod> {
    let mut out: Vec<FeePeriod> = Vec::new();
    if rule.cadence == Cadence::Annual {
        let month0 = rule.charge_month.unwrap_or(12).clamp(1, 12) - 1;
        for year in from.year()..=to.year() {
            let date = charge_date(year, month0, rule.charge_day);
            if date < from || date > to {
                continue;
            }
            // Competence: the 12 months ending at the charge month.
            let (sy, sm0) = from_abs(abs_month(year, month0) - 11);
            out.push(FeePeriod {
                from: first_of_month(sy, sm0),
                to: last_of_month(year, month0),
                date,
            });
        }
    } else {
        let passo = rule.cadence.months_per_period() as i64;
        // Accruing rules are debited in arrears (month after the period ends).
        let lag = i64::from(accrues_daily(rule));
        let start_abs = abs_month(from.year(), from.month0()) - lag;
        let to_abs = abs_month(to.year(), to.month0());
        let mut p = start_abs.div_euclid(passo) * passo;
        loop {
            let period_end_abs = p + passo - 1;
            let charge_abs = period_end_abs + lag;
            if charge_abs > to_abs {
                break;
            }
            let (cy, cm0) = from_abs(charge_abs);
            let (ey, em0) = from_abs(period_end_abs);
            let date = if lag == 1 {
                calendar.next_business_day(first_of_month(cy, cm0))
            } else {
                charge_date(cy, cm0, rule.charge_day)
            };
            if date >= from && date <= to {
                let (py, pm0) = from_abs(p);
                out.push(FeePeriod {
                    from: first_of_month(py, pm0),
                    to: last_of_month(ey, em0),
                    date,
                });
            }
            p += passo;
        }
    }
    out
}

/// The charge date within month `month0` of `year`: `charge_day == 0` means the
/// last day of the month, otherwise `charge_day` clamped to the month's length.
fn charge_date(year: i32, month0: u32, charge_day: u32) -> NaiveDate {
    let last = last_of_month(year, month0).day();
    let day = if charge_day == 0 {
        last
    } else {
        charge_day.min(last).max(1)
    };
    NaiveDate::from_ymd_opt(year, month0 + 1, day).expect("valid charge date")
}

/// The absolute month index (`year * 12 + zero_based_month`) for `year`/`month0`.
fn abs_month(year: i32, month0: u32) -> i64 {
    year as i64 * 12 + month0 as i64
}

/// The `(year, zero_based_month)` for an absolute month index.
fn from_abs(abs: i64) -> (i32, u32) {
    let year = abs / 12;
    let month0 = (abs % 12) as u32;
    (year as i32, month0)
}

/// The first day of `year`/`month0`.
fn first_of_month(year: i32, month0: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month0 + 1, 1).expect("valid month")
}

/// The last day of `year`/`month0`.
fn last_of_month(year: i32, month0: u32) -> NaiveDate {
    let (ny, nm0) = if month0 == 11 {
        (year + 1, 0)
    } else {
        (year, month0 + 1)
    };
    first_of_month(ny, nm0) - Duration::days(1)
}

/// The `[min competence start, max competence end]` span across every deadline
/// the rule would charge inside the horizon `[from, to]`.
///
/// This is the data range the caller must preload (opening balance plus the
/// base-ledger rows) so the per-period magnitudes can be derived without any
/// further I/O. Returns `None` when the rule has no deadline in `[from, to]`,
/// in which case there is nothing to compute or materialize.
pub fn fee_horizon(
    rule: &FeeRule,
    from: NaiveDate,
    to: NaiveDate,
) -> Option<(NaiveDate, NaiveDate)> {
    fee_horizon_in(rule, from, to, &HolidayCalendar::weekends_only())
}

/// Like [`fee_horizon`], honouring `calendar` for arrears-dated rules.
pub fn fee_horizon_in(
    rule: &FeeRule,
    from: NaiveDate,
    to: NaiveDate,
    calendar: &HolidayCalendar,
) -> Option<(NaiveDate, NaiveDate)> {
    let periods = charge_periods(rule, from, to, calendar);
    if periods.is_empty() {
        return None;
    }
    let min_from = periods.iter().map(|p| p.from).min()?;
    let max_to = periods.iter().map(|p| p.to).max()?;
    Some((min_from, max_to))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn const_metrics(
        avg: i64,
        end: i64,
        out: i64,
        in_cents: i64,
        count: i64,
    ) -> impl Fn(&FeePeriod) -> PeriodMetrics {
        move |_p| PeriodMetrics {
            avg_balance_cents: avg,
            end_balance_cents: end,
            total_out_cents: out,
            total_in_cents: in_cents,
            tx_count: count,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn rule(
        calc: CalcType,
        value: i64,
        is_annual: bool,
        cap: Option<i64>,
        cadence: Cadence,
        charge_day: u32,
        charge_month: Option<u32>,
        base: CondBase,
        op: Option<CondOp>,
        threshold: Option<i64>,
    ) -> FeeRule {
        FeeRule {
            calc_type: calc,
            value,
            is_annual,
            prorata_days: false,
            annual_cap_cents: cap,
            cadence,
            charge_day,
            charge_month,
            cond_base: base,
            cond_op: op,
            cond_threshold: threshold,
        }
    }

    // ── charge_day handling across cadences (incl. February) ───────────────

    #[test]
    fn charge_day_zero_monthly_leap_feb() {
        let r = rule(
            CalcType::Fixed,
            1000,
            false,
            None,
            Cadence::Monthly,
            0,
            None,
            CondBase::Always,
            None,
            None,
        );
        let rows = generate_fees(
            &r,
            d("2024-01-01"),
            d("2024-03-31"),
            const_metrics(0, 0, 0, 0, 0),
        );
        let dates: Vec<_> = rows.iter().map(|r| r.date).collect();
        // 2024 is a leap year: Feb has 29 days.
        assert_eq!(
            dates,
            vec![d("2024-01-31"), d("2024-02-29"), d("2024-03-31")]
        );
        assert!(rows.iter().all(|r| r.amount_cents == -1000));
    }

    #[test]
    fn charge_day_zero_monthly_nonleap_feb() {
        let r = rule(
            CalcType::Fixed,
            100,
            false,
            None,
            Cadence::Monthly,
            0,
            None,
            CondBase::Always,
            None,
            None,
        );
        let rows = generate_fees(
            &r,
            d("2023-01-01"),
            d("2023-02-28"),
            const_metrics(0, 0, 0, 0, 0),
        );
        let dates: Vec<_> = rows.iter().map(|r| r.date).collect();
        assert_eq!(dates, vec![d("2023-01-31"), d("2023-02-28")]);
    }

    #[test]
    fn charge_day_clamped_to_month_length() {
        let r = rule(
            CalcType::Fixed,
            100,
            false,
            None,
            Cadence::Monthly,
            31,
            None,
            CondBase::Always,
            None,
            None,
        );
        let rows = generate_fees(
            &r,
            d("2024-01-01"),
            d("2024-03-31"),
            const_metrics(0, 0, 0, 0, 0),
        );
        let dates: Vec<_> = rows.iter().map(|r| r.date).collect();
        // Day 31 is clamped: Feb 2024 -> 29.
        assert_eq!(
            dates,
            vec![d("2024-01-31"), d("2024-02-29"), d("2024-03-31")]
        );
    }

    #[test]
    fn charge_day_zero_various_cadences() {
        // Quarterly: last day of Mar/Jun/Sep/Dec.
        let r = rule(
            CalcType::Fixed,
            1,
            false,
            None,
            Cadence::Quarterly,
            0,
            None,
            CondBase::Always,
            None,
            None,
        );
        let dates: Vec<_> = generate_fees(
            &r,
            d("2024-01-01"),
            d("2024-12-31"),
            const_metrics(0, 0, 0, 0, 0),
        )
        .iter()
        .map(|r| r.date)
        .collect();
        assert_eq!(
            dates,
            vec![
                d("2024-03-31"),
                d("2024-06-30"),
                d("2024-09-30"),
                d("2024-12-31")
            ]
        );

        // Semiannual: last day of Jun/Dec.
        let r = rule(
            CalcType::Fixed,
            1,
            false,
            None,
            Cadence::Semiannual,
            0,
            None,
            CondBase::Always,
            None,
            None,
        );
        let dates: Vec<_> = generate_fees(
            &r,
            d("2024-01-01"),
            d("2024-12-31"),
            const_metrics(0, 0, 0, 0, 0),
        )
        .iter()
        .map(|r| r.date)
        .collect();
        assert_eq!(dates, vec![d("2024-06-30"), d("2024-12-31")]);

        // Bimonthly: last day of Feb/Apr/.../Dec.
        let r = rule(
            CalcType::Fixed,
            1,
            false,
            None,
            Cadence::Bimonthly,
            0,
            None,
            CondBase::Always,
            None,
            None,
        );
        let dates: Vec<_> = generate_fees(
            &r,
            d("2024-01-01"),
            d("2024-12-31"),
            const_metrics(0, 0, 0, 0, 0),
        )
        .iter()
        .map(|r| r.date)
        .collect();
        assert_eq!(
            dates,
            vec![
                d("2024-02-29"),
                d("2024-04-30"),
                d("2024-06-30"),
                d("2024-08-31"),
                d("2024-10-31"),
                d("2024-12-31")
            ]
        );
    }

    #[test]
    fn charge_day_zero_annual_charge_month() {
        // Annual fee charged on the last day of December (calendar year).
        let r = rule(
            CalcType::Fixed,
            1200,
            false,
            None,
            Cadence::Annual,
            0,
            Some(12),
            CondBase::Always,
            None,
            None,
        );
        let rows = generate_fees(
            &r,
            d("2024-01-01"),
            d("2025-12-31"),
            const_metrics(0, 0, 0, 0, 0),
        );
        let dates: Vec<_> = rows.iter().map(|r| r.date).collect();
        assert_eq!(dates, vec![d("2024-12-31"), d("2025-12-31")]);
        assert!(rows.iter().all(|r| r.amount_cents == -1200));
    }

    // ── calc_type coverage ─────────────────────────────────────────────────

    #[test]
    fn calc_fixed() {
        let r = rule(
            CalcType::Fixed,
            3420,
            false,
            None,
            Cadence::Quarterly,
            0,
            None,
            CondBase::Always,
            None,
            None,
        );
        let rows = generate_fees(
            &r,
            d("2024-01-01"),
            d("2024-06-30"),
            const_metrics(0, 0, 0, 0, 0),
        );
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.amount_cents == -3420));
    }

    #[test]
    fn calc_pct_avg_balance() {
        // 0.20% (20 bps) of an average balance of 10,000.00 EUR.
        let r = rule(
            CalcType::PctAvgBalance,
            20,
            false,
            None,
            Cadence::Quarterly,
            0,
            None,
            CondBase::Always,
            None,
            None,
        );
        let rows = generate_fees(
            &r,
            d("2024-01-01"),
            d("2024-03-31"),
            const_metrics(1_000_000, 0, 0, 0, 0),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].amount_cents, -2000);
    }

    #[test]
    fn calc_pct_end_balance() {
        // 0.10% (10 bps) of a closing balance of 50,000.00 EUR.
        let r = rule(
            CalcType::PctEndBalance,
            10,
            false,
            None,
            Cadence::Quarterly,
            0,
            None,
            CondBase::Always,
            None,
            None,
        );
        let rows = generate_fees(
            &r,
            d("2024-01-01"),
            d("2024-03-31"),
            const_metrics(0, 5_000_000, 0, 0, 0),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].amount_cents, -5000);
    }

    #[test]
    fn pct_negative_balance_is_zero() {
        let r = rule(
            CalcType::PctAvgBalance,
            20,
            false,
            None,
            Cadence::Monthly,
            0,
            None,
            CondBase::Always,
            None,
            None,
        );
        let rows = generate_fees(
            &r,
            d("2024-01-01"),
            d("2024-01-31"),
            const_metrics(-1_000_000, 0, 0, 0, 0),
        );
        assert!(rows.is_empty());
    }

    // ── cond_base coverage ─────────────────────────────────────────────────

    #[test]
    fn cond_always() {
        let r = rule(
            CalcType::Fixed,
            100,
            false,
            None,
            Cadence::Monthly,
            0,
            None,
            CondBase::Always,
            None,
            None,
        );
        let rows = generate_fees(
            &r,
            d("2024-01-01"),
            d("2024-01-31"),
            const_metrics(0, 0, 0, 0, 0),
        );
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn cond_avg_balance() {
        let r = rule(
            CalcType::Fixed,
            100,
            false,
            None,
            Cadence::Monthly,
            0,
            None,
            CondBase::AvgBalance,
            Some(CondOp::Gt),
            Some(500_000),
        );
        // Above threshold: charged.
        let rows = generate_fees(
            &r,
            d("2024-01-01"),
            d("2024-01-31"),
            const_metrics(1_000_000, 0, 0, 0, 0),
        );
        assert_eq!(rows.len(), 1);
        // Below threshold: not charged.
        let rows = generate_fees(
            &r,
            d("2024-01-01"),
            d("2024-01-31"),
            const_metrics(400_000, 0, 0, 0, 0),
        );
        assert!(rows.is_empty());
    }

    #[test]
    fn cond_end_balance() {
        let r = rule(
            CalcType::Fixed,
            100,
            false,
            None,
            Cadence::Monthly,
            0,
            None,
            CondBase::EndBalance,
            Some(CondOp::Ge),
            Some(1_000_000),
        );
        let rows = generate_fees(
            &r,
            d("2024-01-01"),
            d("2024-01-31"),
            const_metrics(0, 1_000_000, 0, 0, 0),
        );
        assert_eq!(rows.len(), 1);
        let rows = generate_fees(
            &r,
            d("2024-01-01"),
            d("2024-01-31"),
            const_metrics(0, 999_999, 0, 0, 0),
        );
        assert!(rows.is_empty());
    }

    #[test]
    fn cond_total_out() {
        let r = rule(
            CalcType::Fixed,
            100,
            false,
            None,
            Cadence::Monthly,
            0,
            None,
            CondBase::TotalOut,
            Some(CondOp::Gt),
            Some(200_000),
        );
        let rows = generate_fees(
            &r,
            d("2024-01-01"),
            d("2024-01-31"),
            const_metrics(0, 0, 300_000, 0, 0),
        );
        assert_eq!(rows.len(), 1);
        let rows = generate_fees(
            &r,
            d("2024-01-01"),
            d("2024-01-31"),
            const_metrics(0, 0, 100_000, 0, 0),
        );
        assert!(rows.is_empty());
    }

    #[test]
    fn cond_total_in() {
        let r = rule(
            CalcType::Fixed,
            100,
            false,
            None,
            Cadence::Monthly,
            0,
            None,
            CondBase::TotalIn,
            Some(CondOp::Lt),
            Some(50_000),
        );
        let rows = generate_fees(
            &r,
            d("2024-01-01"),
            d("2024-01-31"),
            const_metrics(0, 0, 0, 10_000, 0),
        );
        assert_eq!(rows.len(), 1);
        let rows = generate_fees(
            &r,
            d("2024-01-01"),
            d("2024-01-31"),
            const_metrics(0, 0, 0, 60_000, 0),
        );
        assert!(rows.is_empty());
    }

    #[test]
    fn cond_tx_count() {
        let r = rule(
            CalcType::Fixed,
            100,
            false,
            None,
            Cadence::Monthly,
            0,
            None,
            CondBase::TxCount,
            Some(CondOp::Gt),
            Some(5),
        );
        let rows = generate_fees(
            &r,
            d("2024-01-01"),
            d("2024-01-31"),
            const_metrics(0, 0, 0, 0, 7),
        );
        assert_eq!(rows.len(), 1);
        let rows = generate_fees(
            &r,
            d("2024-01-01"),
            d("2024-01-31"),
            const_metrics(0, 0, 0, 0, 3),
        );
        assert!(rows.is_empty());
    }

    // ── annual cap ─────────────────────────────────────────────────────────

    #[test]
    fn annual_cap_truncates_cumulative_sum() {
        // 12 monthly charges of 1000 would total 12000; cap at 2500.
        let r = rule(
            CalcType::Fixed,
            1000,
            false,
            Some(2500),
            Cadence::Monthly,
            0,
            None,
            CondBase::Always,
            None,
            None,
        );
        let rows = generate_fees(
            &r,
            d("2024-01-01"),
            d("2024-12-31"),
            const_metrics(0, 0, 0, 0, 0),
        );
        let mags: Vec<i64> = rows.iter().map(|r| -r.amount_cents).collect();
        // Jan 1000, Feb 1000, Mar 500, then truncated to nothing.
        assert_eq!(mags, vec![1000, 1000, 500]);
        assert_eq!(mags.iter().sum::<i64>(), 2500);
    }

    #[test]
    fn annual_cap_not_exceeded_leaves_headroom() {
        // Charges would total 3000, cap 5000: no truncation.
        let r = rule(
            CalcType::Fixed,
            1000,
            false,
            Some(5000),
            Cadence::Quarterly,
            0,
            None,
            CondBase::Always,
            None,
            None,
        );
        let rows = generate_fees(
            &r,
            d("2024-01-01"),
            d("2024-12-31"),
            const_metrics(0, 0, 0, 0, 0),
        );
        let mags: Vec<i64> = rows.iter().map(|r| -r.amount_cents).collect();
        assert_eq!(mags, vec![1000, 1000, 1000, 1000]);
        assert_eq!(mags.iter().sum::<i64>(), 4000);
    }

    // ── is_annual distribution ─────────────────────────────────────────────

    #[test]
    fn is_annual_fixed_distribution_sums_exactly() {
        // 10,000 cents/year over 12 monthly deadlines.
        let r = rule(
            CalcType::Fixed,
            10_000,
            true,
            None,
            Cadence::Monthly,
            0,
            None,
            CondBase::Always,
            None,
            None,
        );
        let rows = generate_fees(
            &r,
            d("2024-01-01"),
            d("2024-12-31"),
            const_metrics(0, 0, 0, 0, 0),
        );
        assert_eq!(rows.len(), 12);
        // 10000/12 = 833 remainder 4 -> eleven 833 and a final 837.
        let mags: Vec<i64> = rows.iter().map(|r| -r.amount_cents).collect();
        assert_eq!(
            mags,
            vec![833, 833, 833, 833, 833, 833, 833, 833, 833, 833, 833, 837]
        );
        assert_eq!(mags.iter().sum::<i64>(), 10_000);
    }

    #[test]
    fn is_annual_distributes_over_charged_deadlines() {
        // Only quarters where the average balance exceeds 5,000,000 are charged;
        // the annual 12,000 is split over those 2 charged deadlines.
        let r = rule(
            CalcType::Fixed,
            12_000,
            true,
            None,
            Cadence::Quarterly,
            0,
            None,
            CondBase::AvgBalance,
            Some(CondOp::Gt),
            Some(5_000_000),
        );
        let rows = generate_fees(&r, d("2024-01-01"), d("2024-12-31"), |p| {
            let avg = match p.to {
                _ if p.to <= d("2024-03-31") => 6_000_000,
                _ if p.to <= d("2024-06-30") => 1_000_000,
                _ if p.to <= d("2024-09-30") => 7_000_000,
                _ => 2_000_000,
            };
            PeriodMetrics {
                avg_balance_cents: avg,
                ..Default::default()
            }
        });
        let mags: Vec<i64> = rows.iter().map(|r| -r.amount_cents).collect();
        assert_eq!(mags, vec![6000, 6000]);
        let dates: Vec<_> = rows.iter().map(|r| r.date).collect();
        assert_eq!(dates, vec![d("2024-03-31"), d("2024-09-30")]);
    }

    // ── prorata_days (stamp duty on actual days, debited in arrears) ───────

    fn stamp_rule(
        cadence: Cadence,
        base: CondBase,
        op: Option<CondOp>,
        thr: Option<i64>,
    ) -> FeeRule {
        FeeRule {
            prorata_days: true,
            ..rule(
                CalcType::Fixed,
                3420,
                true,
                None,
                cadence,
                0,
                None,
                base,
                op,
                thr,
            )
        }
    }

    #[test]
    fn prorata_off_keeps_equal_split_and_period_end_date() {
        let r = FeeRule {
            prorata_days: false,
            ..stamp_rule(Cadence::Quarterly, CondBase::Always, None, None)
        };
        let cal = crate::calendar::fixtures::italy();
        let rows = generate_fees_in(&r, d("2025-01-01"), d("2025-12-31"), &cal, |_| {
            PeriodMetrics::default()
        });
        let mags: Vec<i64> = rows.iter().map(|r| -r.amount_cents).collect();
        assert_eq!(mags, vec![855, 855, 855, 855]);
        assert_eq!(rows[3].date, d("2025-12-31"));
    }

    #[test]
    fn stamp_duty_q4_2024_is_prorata_and_charged_first_working_day_of_january() {
        let r = stamp_rule(
            Cadence::Quarterly,
            CondBase::AvgBalance,
            Some(CondOp::Gt),
            Some(500_000),
        );
        let cal = crate::calendar::fixtures::italy();
        // Only Q4 2024 exceeds the threshold.
        let rows = generate_fees_in(&r, d("2024-01-01"), d("2025-02-28"), &cal, |p| {
            PeriodMetrics {
                avg_balance_cents: if p.from == d("2024-10-01") {
                    900_000
                } else {
                    0
                },
                ..Default::default()
            }
        });
        // 92 days of 366: 3420 * 92 / 366 = 859.67 -> 860 cents; 1 Jan is a
        // holiday, so the first working day is Thursday 2 Jan 2025.
        assert_eq!(
            rows,
            vec![FeeRow {
                date: d("2025-01-02"),
                amount_cents: -860
            }]
        );
    }

    #[test]
    fn stamp_duty_full_year_sums_exactly() {
        let r = stamp_rule(Cadence::Quarterly, CondBase::Always, None, None);
        let cal = crate::calendar::fixtures::italy();
        // Periods ending in 2025 (Q1..Q4), the last one charged on 2026-01-02.
        let rows = generate_fees_in(&r, d("2025-04-01"), d("2026-01-31"), &cal, |_| {
            PeriodMetrics::default()
        });
        let mags: Vec<i64> = rows.iter().map(|r| -r.amount_cents).collect();
        // 90/91/92/92 days of 365: 8.43 + 8.53 + 8.62 + 8.62 = 34.20.
        assert_eq!(mags, vec![843, 853, 862, 862]);
        assert_eq!(mags.iter().sum::<i64>(), 3420);
        let dates: Vec<_> = rows.iter().map(|r| r.date).collect();
        assert_eq!(
            dates,
            vec![
                d("2025-04-01"),
                d("2025-07-01"),
                d("2025-10-01"),
                d("2026-01-02")
            ]
        );
    }

    #[test]
    fn prorata_monthly_sums_exactly_over_leap_year() {
        let r = FeeRule {
            value: 10_000,
            ..stamp_rule(Cadence::Monthly, CondBase::Always, None, None)
        };
        let cal = crate::calendar::fixtures::italy();
        let rows = generate_fees_in(&r, d("2024-02-01"), d("2025-01-31"), &cal, |_| {
            PeriodMetrics::default()
        });
        let mags: Vec<i64> = rows.iter().map(|r| -r.amount_cents).collect();
        assert_eq!(mags.len(), 12);
        assert_eq!(mags.iter().sum::<i64>(), 10_000);
        // Jan 31/366 -> 847, Feb 29/366 cumulative 1639 -> 792.
        assert_eq!(&mags[..2], &[847, 792]);
    }

    // ── misc ───────────────────────────────────────────────────────────────

    #[test]
    fn from_after_to_is_empty() {
        let r = rule(
            CalcType::Fixed,
            100,
            false,
            None,
            Cadence::Monthly,
            0,
            None,
            CondBase::Always,
            None,
            None,
        );
        assert!(generate_fees(
            &r,
            d("2024-02-01"),
            d("2024-01-01"),
            const_metrics(0, 0, 0, 0, 0)
        )
        .is_empty());
    }

    #[test]
    fn zero_value_produces_no_rows() {
        let r = rule(
            CalcType::Fixed,
            0,
            false,
            None,
            Cadence::Monthly,
            0,
            None,
            CondBase::Always,
            None,
            None,
        );
        assert!(generate_fees(
            &r,
            d("2024-01-01"),
            d("2024-01-31"),
            const_metrics(0, 0, 0, 0, 0)
        )
        .is_empty());
    }

    // ── parse / as_str round-trips ─────────────────────────────────────────

    #[test]
    fn enum_round_trips() {
        for c in [
            CalcType::Fixed,
            CalcType::PctAvgBalance,
            CalcType::PctEndBalance,
        ] {
            assert_eq!(CalcType::parse(c.as_str()), Some(c));
        }
        for c in [
            Cadence::Monthly,
            Cadence::Bimonthly,
            Cadence::Quarterly,
            Cadence::Semiannual,
            Cadence::Annual,
        ] {
            assert_eq!(Cadence::parse(c.as_str()), Some(c));
        }
        for c in [
            CondBase::Always,
            CondBase::AvgBalance,
            CondBase::EndBalance,
            CondBase::TotalOut,
            CondBase::TotalIn,
            CondBase::TxCount,
        ] {
            assert_eq!(CondBase::parse(c.as_str()), Some(c));
        }
        for c in [CondOp::Gt, CondOp::Ge, CondOp::Lt, CondOp::Le] {
            assert_eq!(CondOp::parse(c.as_str()), Some(c));
        }
        assert_eq!(CalcType::parse("nope"), None);
        assert_eq!(Cadence::parse("nope"), None);
        assert_eq!(CondBase::parse("nope"), None);
        assert_eq!(CondOp::parse("nope"), None);
    }
}
