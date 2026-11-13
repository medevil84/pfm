//! Pure credit-card settlement engine.
//!
//! A card account accrues raw movements during a statement cycle (the raw
//! ledger rows are `pair_id IS NULL`): spends are negative, refunds/credits
//! positive. Two configurable days drive the settlement:
//!
//! - `closing_day` (giorno di chiusura contabile): the day of the month the
//!   statement cycle closes. Cycles tile the timeline with no overlap and no
//!   gap: `(closing_day of previous month, exclusive] .. closing_day of this
//!   month, inclusive]`.
//! - `charge_day` (giorno di addebito): the day of the month the backing
//!   account is actually debited for a closed cycle's total. The charge is
//!   always dated *after* (or on) the cycle's closing date:
//!     - if `charge_day >= closing_day`, the charge falls in the **same**
//!       month as the closing date;
//!     - if `charge_day < closing_day`, the charge falls in the **following**
//!       month.
//!
//! On the charge day the backing (primary) account is debited for the *sum*
//! of the cycle's raw outflows and the card is credited with the same amount,
//! as a single linked pair. There is never a fixed amount: the charge is
//! exactly what the cycle consumed.
//!
//! This module is intentionally free of I/O, serialization, or framework
//! dependencies. It reasons only about dates and the already-fetched rows of a
//! cycle; the service layer loads them and persists the settlement pair.

use chrono::{Datelike, Duration, NaiveDate, Weekday};

use crate::calendar::HolidayCalendar;

use crate::Transaction;

/// One settlement cycle of a card.
///
/// The billed window is the half-open interval `(cycle_start, closing_date]`:
/// consecutive cycles tile the timeline with no overlap and no gap. The card is
/// credited (reset to zero) on `closing_date`; the backing account is debited on
/// `charge_date` (which is on or after `closing_date`, and — at the service
/// layer — shifted forward to the next business day when it lands on a
/// weekend/holiday).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cycle {
    /// Exclusive lower bound of the billed window (the previous cycle's closing
    /// date), never earlier than the account's opening date.
    pub cycle_start: NaiveDate,
    /// The accounting closing date (`closing_day`): inclusive upper bound of the
    /// billed window, and the date the card credit (reset) is posted.
    pub closing_date: NaiveDate,
    /// The nominal debit date (`charge_day`) on the backing account.
    pub charge_date: NaiveDate,
}

impl Cycle {
    /// Exclusive lower bound of the movements actually billed in this cycle.
    pub fn window_start(&self) -> NaiveDate {
        effective_close(self.cycle_start)
    }

    /// Inclusive upper bound of the movements actually billed in this cycle.
    pub fn window_end(&self) -> NaiveDate {
        effective_close(self.closing_date)
    }
}

/// The last day whose movements belong to the cycle closing on `closing_date`.
///
/// Movements dated on the closing day normally close the current cycle, but when
/// the closing day falls on a Saturday or Sunday the whole weekend rolls into the
/// *next* cycle: the statement is cut on the preceding Friday.
pub fn effective_close(closing_date: NaiveDate) -> NaiveDate {
    match closing_date.weekday() {
        Weekday::Sat => closing_date - Duration::days(1),
        Weekday::Sun => closing_date - Duration::days(2),
        _ => closing_date,
    }
}

/// The end-of-cycle charge amount owed on the card, in cents (always `>= 0`).
///
/// `cycle` is the list of the previous cycle's raw card movements (spends
/// negative, credits positive). The charge is the net outflow: if the cycle's
/// total is negative the card holder owes its absolute value; if the cycle nets
/// to zero or positive (fully refunded / over-credited) nothing is charged.
///
/// The summation is done in `i128` so a large cycle cannot overflow `i64`.
pub fn charge_amount(cycle: &[Transaction]) -> i64 {
    let sum: i128 = cycle.iter().map(|t| t.cents as i128).sum();
    let owed = -sum;
    if owed > 0 {
        owed.clamp(i64::MIN as i128, i64::MAX as i128) as i64
    } else {
        0
    }
}

/// The first day of the month preceding the month containing `d`.
fn first_of_prev_month(d: NaiveDate) -> NaiveDate {
    let first_of_this =
        NaiveDate::from_ymd_opt(d.year(), d.month(), 1).expect("day 1 is always valid");
    first_of_this - Duration::days(1)
}

/// The first day of the month following the month containing `d`.
fn first_of_next_month(d: NaiveDate) -> NaiveDate {
    if d.month() == 12 {
        NaiveDate::from_ymd_opt(d.year() + 1, 1, 1).expect("day 1 is always valid")
    } else {
        NaiveDate::from_ymd_opt(d.year(), d.month() + 1, 1).expect("day 1 is always valid")
    }
}

/// The first occurrence of `day` (day-of-month, `1..=28`) on or after `start`.
fn first_day_on_or_after(day: i32, start: NaiveDate) -> NaiveDate {
    if start.day() as i32 <= day {
        NaiveDate::from_ymd_opt(start.year(), start.month(), day as u32)
            .expect("day 1..=28 is always valid")
    } else {
        let next = if start.month() == 12 {
            NaiveDate::from_ymd_opt(start.year() + 1, 1, day as u32)
        } else {
            NaiveDate::from_ymd_opt(start.year(), start.month() + 1, day as u32)
        };
        next.expect("day 1..=28 is always valid")
    }
}

/// The most recent occurrence of `day` (day-of-month) on or before `as_of`.
fn most_recent_day_on_or_before(day: i32, as_of: NaiveDate) -> Option<NaiveDate> {
    if as_of.day() as i32 >= day {
        NaiveDate::from_ymd_opt(as_of.year(), as_of.month(), day as u32)
    } else {
        let prev = first_of_prev_month(as_of);
        NaiveDate::from_ymd_opt(prev.year(), prev.month(), day as u32)
    }
}

/// Advance `d` (which is on some fixed day-of-month `1..=28`) by one month to
/// the next occurrence of that same day.
fn next_month_same_day(d: NaiveDate) -> NaiveDate {
    if d.month() == 12 {
        NaiveDate::from_ymd_opt(d.year() + 1, 1, d.day()).expect("day 1..=28 always valid")
    } else {
        NaiveDate::from_ymd_opt(d.year(), d.month() + 1, d.day()).expect("day 1..=28 always valid")
    }
}

/// Given a cycle's `closing_date` (already on `closing_day`), the date the
/// cycle is actually charged: the first occurrence of `charge_day` that is
/// not before the closing date. Same month when `charge_day >= closing_day`,
/// otherwise the following month.
fn charge_date_from_closing(
    closing_date: NaiveDate,
    closing_day: i32,
    charge_day: i32,
) -> NaiveDate {
    if charge_day >= closing_day {
        NaiveDate::from_ymd_opt(closing_date.year(), closing_date.month(), charge_day as u32)
            .expect("charge_day 1..=28 is always valid")
    } else {
        let next = first_of_next_month(closing_date);
        NaiveDate::from_ymd_opt(next.year(), next.month(), charge_day as u32)
            .expect("charge_day 1..=28 is always valid")
    }
}

/// The inverse of [`charge_date_from_closing`]: given a cycle's `charge_date`
/// (already on `charge_day`), the closing date it bills.
fn closing_date_from_charge(
    charge_date: NaiveDate,
    closing_day: i32,
    charge_day: i32,
) -> NaiveDate {
    if charge_day >= closing_day {
        NaiveDate::from_ymd_opt(charge_date.year(), charge_date.month(), closing_day as u32)
            .expect("closing_day 1..=28 is always valid")
    } else {
        let prev = first_of_prev_month(charge_date);
        NaiveDate::from_ymd_opt(prev.year(), prev.month(), closing_day as u32)
            .expect("closing_day 1..=28 is always valid")
    }
}

/// The settlement cycle to bill for a card, given a "today" date.
///
/// Returns `(charge_date, cycle_start)` or `None` when no charge is due yet.
///
/// - `charge_date` is the most recent date `<= as_of` whose day-of-month equals
///   `charge_day`. Because callers constrain `charge_day` to `1..=28`, that day
///   exists in every month, so the date is always well-formed.
/// - `cycle_start` is the closing date (`closing_day`) of the month preceding
///   the cycle this charge bills, clamped up to `account_start` (a card opened
///   mid-cycle bills only from its opening date on its first settlement).
/// - The billed window is the half-open interval `(cycle_start, closing_date]`:
///   consecutive cycles tile the timeline with no overlap and no gap.
///
/// Returns `None` when the cycle this charge would bill closed before the
/// account was opened (the card has not yet reached its first closing).
pub fn cycle_bounds(
    closing_day: i32,
    charge_day: i32,
    as_of: NaiveDate,
    account_start: NaiveDate,
) -> Option<(NaiveDate, NaiveDate)> {
    let charge_date = most_recent_day_on_or_before(charge_day, as_of)?;
    let closing_date = closing_date_from_charge(charge_date, closing_day, charge_day);

    // A cycle closed before the account existed cannot be billed.
    if closing_date < account_start {
        return None;
    }

    // The previous cycle begins on the same closing_day of the preceding
    // month, never before the account opened.
    let prev = first_of_prev_month(closing_date);
    let cycle_start = NaiveDate::from_ymd_opt(prev.year(), prev.month(), closing_day as u32)?;
    let cycle_start = cycle_start.max(account_start);

    Some((charge_date, cycle_start))
}

/// Determine the settlement cycle that a transaction on `date` belongs to.
///
/// A transaction belongs to the cycle whose window `(cycle_start, closing_date]`
/// contains it: the boundary day itself (`date.day() == closing_day`) closes the
/// *current* cycle, so it accrues here, not into the next one. Returns `None` if
/// the cycle it would close into precedes `account_start` (the card had not yet
/// opened).
pub fn cycle_for_date(
    closing_day: i32,
    charge_day: i32,
    date: NaiveDate,
    account_start: NaiveDate,
) -> Option<Cycle> {
    let this_closing = NaiveDate::from_ymd_opt(date.year(), date.month(), closing_day as u32)?;
    let closing_date = if date > effective_close(this_closing) {
        // Transaction date is after this month's cut-off (the closing day, or
        // the day before when it is a weekend): it accrues into the cycle that
        // closes the FOLLOWING month.
        let next = first_of_next_month(date);
        NaiveDate::from_ymd_opt(next.year(), next.month(), closing_day as u32)?
    } else {
        // Transaction date is on or before the cut-off: it accrues into the
        // cycle that closes THIS month.
        this_closing
    };

    if closing_date < account_start {
        return None;
    }

    // The exclusive lower bound is the previous cycle's closing date, NOT clamped
    // up to `account_start`: because the window is half-open `(cycle_start,
    // closing_date]`, clamping to the opening date would drop movements dated on
    // the opening day itself. No movement can predate the first generated cycle's
    // lower bound, so leaving it unclamped never over-bills.
    let prev = first_of_prev_month(closing_date);
    let cycle_start = NaiveDate::from_ymd_opt(prev.year(), prev.month(), closing_day as u32)?;

    let charge_date = charge_date_from_closing(closing_date, closing_day, charge_day);

    Some(Cycle {
        cycle_start,
        closing_date,
        charge_date,
    })
}

/// All settlement cycles due on or before `as_of`, from the card's opening.
///
/// Returns a sorted `Vec` of [`Cycle`]s, each a billable cycle over the window
/// `(cycle_start, closing_date]`. The first cycle may be truncated by
/// `account_start`. A cycle is "due" once its card reset date (`effective_close(closing_date)`)
/// is `<= as_of`; its backing debit may still lie in the future.
pub fn all_due_cycles(
    closing_day: i32,
    charge_day: i32,
    as_of: NaiveDate,
    account_start: NaiveDate,
) -> Vec<Cycle> {
    let first_closing = first_day_on_or_after(closing_day, account_start);

    let mut cycles = Vec::new();
    let mut closing_date = first_closing;
    loop {
        let charge_date = charge_date_from_closing(closing_date, closing_day, charge_day);
        // A cycle is due once the card has been reset, i.e. on its (weekend-
        // adjusted) closing date; the backing debit keeps its later charge date.
        if effective_close(closing_date) > as_of {
            break;
        }
        // Exclusive lower bound = previous closing date, unclamped (see
        // `cycle_for_date`): the half-open window would otherwise drop movements
        // dated on the opening day.
        let prev = first_of_prev_month(closing_date);
        let cycle_start = NaiveDate::from_ymd_opt(prev.year(), prev.month(), closing_day as u32)
            .expect("closing_day 1..=28 is always valid");
        cycles.push(Cycle {
            cycle_start,
            closing_date,
            charge_date,
        });
        closing_date = next_month_same_day(closing_date);
    }
    cycles
}

/// Shift `date` forward to the next banking business day of `calendar`:
/// weekends and the calendar's public holidays are skipped. Used for the backing
/// account's debit date, which a bank posts on the first working day on or after
/// the nominal charge day. The accounting closing date (the card's own reset) is
/// never shifted.
pub fn next_business_day_in(calendar: &HolidayCalendar, date: NaiveDate) -> NaiveDate {
    calendar.next_business_day(date)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    /// A minimal raw card movement (a spend when `cents < 0`).
    fn tx(cents: i64, date: &str) -> Transaction {
        Transaction {
            id: String::new(),
            client_id: "c".into(),
            account_id: "card".into(),
            date: d(date),
            cents,
            description: None,
            category_id: None,
            note: None,
            pair_id: None,
            rec_id: None,
            rule_id: None,
            override_: None,
            computed: None,
            vacct_id: None,
            import_hash: None,
            created_by: "u".into(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    // ── charge_amount ───────────────────────────────────────────────────────

    #[test]
    fn charge_amount_is_sum_of_outflows() {
        // -10000 and -2500 net outflow -> charge 12500.
        let cycle = vec![tx(-10_000, "2024-06-02"), tx(-2_500, "2024-06-20")];
        assert_eq!(charge_amount(&cycle), 12_500);
    }

    #[test]
    fn charge_amount_credits_reduce_the_charge() {
        // -10000 spend, +2000 refund -> net -8000 -> charge 8000.
        let cycle = vec![tx(-10_000, "2024-06-02"), tx(2_000, "2024-06-18")];
        assert_eq!(charge_amount(&cycle), 8_000);
    }

    #[test]
    fn charge_amount_zero_when_cycle_is_empty() {
        assert_eq!(charge_amount(&[]), 0);
    }

    #[test]
    fn charge_amount_zero_when_fully_refunded() {
        // Spends fully offset by credits -> net zero -> no charge.
        let cycle = vec![tx(-5_000, "2024-06-02"), tx(5_000, "2024-06-18")];
        assert_eq!(charge_amount(&cycle), 0);
    }

    #[test]
    fn charge_amount_zero_when_cycle_nets_positive() {
        // Over-credited: net inflow -> nothing owed.
        let cycle = vec![tx(-5_000, "2024-06-02"), tx(9_000, "2024-06-18")];
        assert_eq!(charge_amount(&cycle), 0);
    }

    // ── cycle_bounds (closing_day == charge_day: legacy single-day behavior) ──

    #[test]
    fn bounds_charge_in_current_month_when_past_charge_day() {
        // closing/charge day 5, as_of 2024-06-20 -> charge 2024-06-05, cycle from 2024-05-05.
        let (c, s) = cycle_bounds(5, 5, d("2024-06-20"), d("2024-01-01")).unwrap();
        assert_eq!(c, d("2024-06-05"));
        assert_eq!(s, d("2024-05-05"));
    }

    #[test]
    fn bounds_charge_in_previous_month_when_before_charge_day() {
        // closing/charge day 5, as_of 2024-06-03 (before the 5th) -> charge 2024-05-05,
        // cycle from 2024-04-05.
        let (c, s) = cycle_bounds(5, 5, d("2024-06-03"), d("2024-01-01")).unwrap();
        assert_eq!(c, d("2024-05-05"));
        assert_eq!(s, d("2024-04-05"));
    }

    #[test]
    fn bounds_cycle_start_clamped_to_account_start() {
        // Account opened 2024-06-15; its first settlement on 2024-07-05 bills
        // only from the opening date, not from 2024-06-05.
        let (c, s) = cycle_bounds(5, 5, d("2024-07-10"), d("2024-06-15")).unwrap();
        assert_eq!(c, d("2024-07-05"));
        assert_eq!(s, d("2024-06-15"));
    }

    #[test]
    fn bounds_none_before_first_charge_day() {
        // Opened 2024-06-15, closing/charge day 5. On 2024-06-20 the only
        // candidate charge (2024-05-05) closed before the opening date ->
        // nothing to bill yet.
        assert!(cycle_bounds(5, 5, d("2024-06-20"), d("2024-06-15")).is_none());
    }

    #[test]
    fn bounds_crosses_year_boundary() {
        // closing/charge day 28, as_of 2024-01-10 -> charge 2023-12-28, cycle
        // from 2023-11-28.
        let (c, s) = cycle_bounds(28, 28, d("2024-01-10"), d("2023-01-01")).unwrap();
        assert_eq!(c, d("2023-12-28"));
        assert_eq!(s, d("2023-11-28"));
    }

    #[test]
    fn bounds_on_the_charge_day_itself() {
        // as_of exactly on the charge day bills the cycle ending that day.
        let (c, s) = cycle_bounds(5, 5, d("2024-06-05"), d("2024-01-01")).unwrap();
        assert_eq!(c, d("2024-06-05"));
        assert_eq!(s, d("2024-05-05"));
    }

    // ── cycle_bounds (closing_day != charge_day) ──────────────────────────────

    #[test]
    fn bounds_charge_day_after_closing_day_same_month() {
        // closing_day 4, charge_day 18 (charge_day >= closing_day -> same month).
        // On 2024-06-25, the most recent charge day (18) is 2024-06-18, which
        // bills the cycle closing 2024-06-04, started 2024-05-04.
        let (c, s) = cycle_bounds(4, 18, d("2024-06-25"), d("2024-01-01")).unwrap();
        assert_eq!(c, d("2024-06-18"));
        assert_eq!(s, d("2024-05-04"));
    }

    #[test]
    fn bounds_charge_day_before_closing_day_next_month() {
        // closing_day 18, charge_day 4 (charge_day < closing_day -> next month).
        // On 2024-07-10, the most recent occurrence of day 4 is 2024-07-04,
        // which bills the cycle closing 2024-06-18 (the previous month),
        // started 2024-05-18.
        let (c, s) = cycle_bounds(18, 4, d("2024-07-10"), d("2024-01-01")).unwrap();
        assert_eq!(c, d("2024-07-04"));
        assert_eq!(s, d("2024-05-18"));
    }

    // ── cycle_for_date / all_due_cycles (worked example from the spec) ────────

    #[test]
    fn cycle_for_date_closing_18_charge_4_spend_before_closing() {
        // closing_day 18, charge_day 4. A spend on the 10th (before the 18th)
        // accrues into the cycle closing THIS month (the 18th), charged the
        // 4th of the month after that.
        let c = cycle_for_date(18, 4, d("2024-06-10"), d("2024-01-01")).unwrap();
        assert_eq!(c.cycle_start, d("2024-05-18"));
        assert_eq!(c.closing_date, d("2024-06-18"));
        assert_eq!(c.charge_date, d("2024-07-04"));
    }

    #[test]
    fn cycle_for_date_closing_18_charge_4_spend_after_closing() {
        // A spend on the 20th (after the 18th) accrues into the cycle closing
        // NEXT month (July 18th), charged August 4th.
        let c = cycle_for_date(18, 4, d("2024-06-20"), d("2024-01-01")).unwrap();
        assert_eq!(c.cycle_start, d("2024-06-18"));
        assert_eq!(c.closing_date, d("2024-07-18"));
        assert_eq!(c.charge_date, d("2024-08-04"));
    }

    #[test]
    fn all_due_cycles_closing_18_charge_4() {
        // Account opened 2024-01-01. As of 2024-07-04 (a charge day), the
        // cycle closing 2024-06-18 (started 2024-05-18) is due, charged
        // 2024-07-04.
        let cycles = all_due_cycles(18, 4, d("2024-07-04"), d("2024-01-01"));
        let last = *cycles.last().unwrap();
        assert_eq!(last.cycle_start, d("2024-05-18"));
        assert_eq!(last.closing_date, d("2024-06-18"));
        assert_eq!(last.charge_date, d("2024-07-04"));
    }

    // ── billed window is (cycle_start, closing_date], not up to charge_date ────

    #[test]
    fn cycle_window_ends_at_closing_not_charge() {
        // Visa case: closing_day 4, charge_day 17. The December cycle closes
        // 2026-01-04 and is charged 2026-01-17. A spend on 2026-01-10 (between
        // the close and the charge) belongs to the NEXT cycle (closing
        // 2026-02-04), never to December's.
        let dec = cycle_for_date(4, 17, d("2025-12-20"), d("2025-01-01")).unwrap();
        assert_eq!(dec.cycle_start, d("2025-12-04"));
        assert_eq!(dec.closing_date, d("2026-01-04"));
        assert_eq!(dec.charge_date, d("2026-01-17"));

        let mid = cycle_for_date(4, 17, d("2026-01-10"), d("2025-01-01")).unwrap();
        assert_eq!(mid.closing_date, d("2026-02-04"));
        assert_ne!(mid.closing_date, dec.closing_date);
    }

    #[test]
    fn weekend_closing_day_rolls_whole_weekend_into_next_cycle() {
        // Closing on Saturday 2026-07-04: Sat and Sun go to the August cycle,
        // Friday the 3rd still closes July.
        let sat = cycle_for_date(4, 17, d("2026-07-04"), d("2026-01-01")).unwrap();
        assert_eq!(sat.closing_date, d("2026-08-04"));
        let fri = cycle_for_date(4, 17, d("2026-07-03"), d("2026-01-01")).unwrap();
        assert_eq!(fri.closing_date, d("2026-07-04"));
        assert_eq!(fri.window_end(), d("2026-07-03"));
        assert_eq!(sat.window_start(), d("2026-07-03"));

        // Closing on Sunday 2026-10-04: Saturday the 3rd moves too, Friday the
        // 2nd is the last day of the September cycle.
        let sat3 = cycle_for_date(4, 17, d("2026-10-03"), d("2026-01-01")).unwrap();
        assert_eq!(sat3.closing_date, d("2026-11-04"));
        let sun4 = cycle_for_date(4, 17, d("2026-10-04"), d("2026-01-01")).unwrap();
        assert_eq!(sun4.closing_date, d("2026-11-04"));
        let fri2 = cycle_for_date(4, 17, d("2026-10-02"), d("2026-01-01")).unwrap();
        assert_eq!(fri2.closing_date, d("2026-10-04"));
        assert_eq!(fri2.window_end(), d("2026-10-02"));
    }

    // ── next_business_day (weekends + Italian national holidays) ──────────────

    #[test]
    fn business_day_shifts_saturday_to_monday() {
        // 2026-01-17 is a Saturday → Monday 2026-01-19.
        assert_eq!(
            next_business_day_in(&crate::calendar::fixtures::italy(), d("2026-01-17")),
            d("2026-01-19")
        );
    }

    #[test]
    fn business_day_skips_fixed_holiday() {
        // 2026-01-06 (Epifania) is a Tuesday → next working day 2026-01-07.
        assert_eq!(
            next_business_day_in(&crate::calendar::fixtures::italy(), d("2026-01-06")),
            d("2026-01-07")
        );
    }

    #[test]
    fn business_day_skips_easter_monday() {
        // Easter 2026 is 2026-04-05; Easter Monday 2026-04-06 → 2026-04-07.
        assert_eq!(
            next_business_day_in(&crate::calendar::fixtures::italy(), d("2026-04-06")),
            d("2026-04-07")
        );
    }

    #[test]
    fn business_day_passes_through_working_day() {
        // 2026-01-19 is a Monday, no holiday → unchanged.
        assert_eq!(
            next_business_day_in(&crate::calendar::fixtures::italy(), d("2026-01-19")),
            d("2026-01-19")
        );
    }
}
