//! Pure recurrence engine: generates a list of dated occurrences from a
//! recurrence pattern, applying weekend policy and amount computation.
//!
//! This module is intentionally free of I/O, serialization, or framework
//! dependencies.

use crate::calendar::HolidayCalendar;
use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Weekday};
use rrule::{Frequency as RRuleFrequency, NWeekday, RRule, RRuleSet, Tz};

/// The frequency of a recurrence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frequency {
    /// Once per year.
    Yearly,
    /// Once per month.
    Monthly,
    /// Once per week.
    Weekly,
    /// Once per day.
    Daily,
}

impl Frequency {
    fn to_rrule(self) -> RRuleFrequency {
        match self {
            Frequency::Yearly => RRuleFrequency::Yearly,
            Frequency::Monthly => RRuleFrequency::Monthly,
            Frequency::Weekly => RRuleFrequency::Weekly,
            Frequency::Daily => RRuleFrequency::Daily,
        }
    }
}

/// Policy for handling occurrences that fall on a weekend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeekendPolicy {
    /// Occurrences on weekends are created as-is.
    Create,
    /// Occurrences on weekends are skipped (not created).
    Skip,
    /// Saturday moves to the preceding Friday; Sunday moves to the preceding Friday.
    Friday,
    /// Saturday moves to the following Monday; Sunday moves to the following Monday.
    Monday,
}

impl WeekendPolicy {
    /// Stable string form used on the wire and in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            WeekendPolicy::Create => "create",
            WeekendPolicy::Skip => "skip",
            WeekendPolicy::Friday => "friday",
            WeekendPolicy::Monday => "monday",
        }
    }

    /// Parse a weekend policy from its string form. `None` for unknown values.
    pub fn parse(s: &str) -> Option<WeekendPolicy> {
        Some(match s {
            "create" => WeekendPolicy::Create,
            "skip" => WeekendPolicy::Skip,
            "friday" => WeekendPolicy::Friday,
            "monday" => WeekendPolicy::Monday,
            _ => return None,
        })
    }
}

impl std::fmt::Display for WeekendPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How the recurrence ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndSpec {
    /// The recurrence never ends.
    Never,
    /// The recurrence ends on or before the given date (inclusive).
    Until(NaiveDate),
    /// The recurrence ends after the given number of total occurrences.
    Count(u32),
}

/// How the amount for each occurrence is determined.
#[derive(Debug, Clone)]
pub enum AmountMode {
    /// Every occurrence has a fixed amount.
    Fixed { amount_cents: i64 },
    /// Each occurrence's amount is the average (round-half-even) of the
    /// last `avg_n` real (historical) occurrences that strictly precede it.
    /// If fewer than `avg_n` precede it, all available are used.
    /// If no real occurrences precede it, `fallback_amount_cents` (the value
    /// provided when the rule was created) is used instead.
    Avg {
        avg_n: u32,
        real_occurrences: Vec<RealOccurrence>,
        fallback_amount_cents: i64,
    },
}

/// A historical (real) occurrence used for average-based amount computation.
#[derive(Debug, Clone)]
pub struct RealOccurrence {
    /// The date of the real occurrence.
    pub date: NaiveDate,
    /// The actual amount in cents.
    pub amount_cents: i64,
}

/// A day-of-week selector for BYDAY, with an optional ordinal.
///
/// `ordinal`: `None` = all, `Some(1)` = first, `Some(-1)` = last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByDay {
    /// Positional ordinal within the period (1=first, -1=last, None=all).
    pub ordinal: Option<i8>,
    /// The day of the week.
    pub weekday: Weekday,
}

/// The full recurrence specification.
#[derive(Debug, Clone)]
pub struct RecurrenceSpec {
    /// The base frequency of the recurrence.
    pub frequency: Frequency,
    /// The interval between occurrences (1 = every period, 2 = every other, etc.).
    pub interval: u16,
    /// Days of the month to include (e.g. `[1, 15]`; negative counts from the end,
    /// `-1` = last day of the month). Empty means "use the start date's day" for
    /// monthly/yearly frequencies.
    pub by_monthday: Vec<i32>,
    /// Day-of-week rules (e.g. first Monday, last Friday). Empty means "use
    /// the start date's weekday" for weekly frequency.
    pub by_weekday: Vec<ByDay>,
    /// How to handle occurrences that land on a weekend.
    pub weekend_policy: WeekendPolicy,
    /// The first date of the recurrence (DTSTART).
    pub first_date: NaiveDate,
    /// How/when the recurrence ends.
    pub end: EndSpec,
    /// How the amount for each occurrence is computed.
    pub amount: AmountMode,
}

/// A single generated occurrence: a date and its amount.
#[derive(Debug, Clone)]
pub struct Occurrence {
    /// The date of the occurrence.
    pub date: NaiveDate,
    /// The amount in cents for this occurrence.
    pub amount_cents: i64,
}

/// Generate occurrences for the given recurrence spec within the `[from, to]` range.
///
/// Returns an empty vec if `from > to` or the spec is invalid.
/// Results are sorted by date and deduplicated.
pub fn generate_occurrences(
    spec: &RecurrenceSpec,
    from: NaiveDate,
    to: NaiveDate,
) -> Vec<Occurrence> {
    generate_occurrences_in(spec, from, to, &HolidayCalendar::weekends_only())
}

/// Like [`generate_occurrences`], but the weekend policy also treats the public
/// holidays of `calendar` as non-business days: `Skip` drops them, `Friday`
/// moves back to the previous business day and `Monday` forward to the next.
pub fn generate_occurrences_in(
    spec: &RecurrenceSpec,
    from: NaiveDate,
    to: NaiveDate,
    calendar: &HolidayCalendar,
) -> Vec<Occurrence> {
    if from > to {
        return Vec::new();
    }

    let dt_start = date_to_dt(spec.first_date);
    // The policy moves a raw date by a few days (a weekend plus adjacent
    // holidays, e.g. Christmas), so the raw window is widened by `SHIFT_MARGIN`
    // on both sides: a raw date just outside `from..=to` can still land inside
    // it once shifted (and vice versa). The final filter below applies the exact
    // window to the shifted dates.
    let after_dt = date_to_dt(from - Duration::days(SHIFT_MARGIN_DAYS));
    let before_dt = date_to_end_of_day(to + Duration::days(SHIFT_MARGIN_DAYS));

    let mut rrule = RRule::new(spec.frequency.to_rrule());
    rrule = rrule.interval(spec.interval);

    if !spec.by_monthday.is_empty() {
        let mdays: Vec<i8> = spec
            .by_monthday
            .iter()
            .filter_map(|d| i8::try_from(*d).ok())
            .collect();
        if !mdays.is_empty() {
            rrule = rrule.by_month_day(mdays);
        }
    }

    if !spec.by_weekday.is_empty() {
        let nwds: Vec<NWeekday> = spec
            .by_weekday
            .iter()
            .map(|bd| NWeekday::new(bd.ordinal.map(|o| o as i16), bd.weekday))
            .collect();
        rrule = rrule.by_weekday(nwds);
    }

    match &spec.end {
        EndSpec::Never => {}
        EndSpec::Until(date) => {
            rrule = rrule.until(date_to_end_of_day(*date));
        }
        EndSpec::Count(n) => {
            rrule = rrule.count(*n);
        }
    }

    let validated = match rrule.validate(dt_start) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let rset = RRuleSet::new(dt_start)
        .rrule(validated)
        .after(after_dt)
        .before(before_dt);
    let raw_dates: Vec<NaiveDate> = rset
        .all_unchecked()
        .into_iter()
        .map(|dt| dt.naive_utc().date())
        .collect();

    let shifted = apply_weekend_policy(raw_dates, spec.weekend_policy, calendar);

    let deduped: Vec<NaiveDate> = {
        let mut seen = std::collections::BTreeSet::new();
        shifted.into_iter().filter(|d| seen.insert(*d)).collect()
    };

    deduped
        .into_iter()
        .filter(|d| *d >= from && *d <= to)
        .map(|date| Occurrence {
            date,
            amount_cents: compute_amount(&spec.amount, date),
        })
        .collect()
}

fn date_to_dt(d: NaiveDate) -> DateTime<Tz> {
    Tz::UTC
        .with_ymd_and_hms(d.year(), d.month(), d.day(), 0, 0, 0)
        .unwrap()
}

fn date_to_end_of_day(d: NaiveDate) -> DateTime<Tz> {
    Tz::UTC
        .with_ymd_and_hms(d.year(), d.month(), d.day(), 23, 59, 59)
        .unwrap()
}

/// Largest shift the weekend/holiday policy can apply to a raw date.
pub const SHIFT_MARGIN_DAYS: i64 = 7;

fn apply_weekend_policy(
    dates: Vec<NaiveDate>,
    policy: WeekendPolicy,
    calendar: &HolidayCalendar,
) -> Vec<NaiveDate> {
    match policy {
        WeekendPolicy::Create => dates,
        WeekendPolicy::Skip => dates
            .into_iter()
            .filter(|d| !calendar.is_non_business_day(*d))
            .collect(),
        WeekendPolicy::Friday => dates
            .into_iter()
            .map(|d| calendar.previous_business_day(d))
            .collect(),
        WeekendPolicy::Monday => dates
            .into_iter()
            .map(|d| calendar.next_business_day(d))
            .collect(),
    }
}

fn compute_amount(mode: &AmountMode, date: NaiveDate) -> i64 {
    match mode {
        AmountMode::Fixed { amount_cents } => *amount_cents,
        AmountMode::Avg {
            avg_n,
            real_occurrences,
            fallback_amount_cents,
        } => {
            if real_occurrences.is_empty() {
                return *fallback_amount_cents;
            }
            let mut count = 0u32;
            let mut sum: i64 = 0;
            for r in real_occurrences.iter().rev() {
                if r.date < date {
                    sum += r.amount_cents;
                    count += 1;
                    if count == *avg_n {
                        break;
                    }
                }
            }
            if count == 0 {
                *fallback_amount_cents
            } else {
                round_half_even(sum, count as i64)
            }
        }
    }
}

fn round_half_even(numerator: i64, denominator: i64) -> i64 {
    if denominator == 0 {
        return 0;
    }
    let q = numerator / denominator;
    let r = numerator % denominator;
    let twice_abs_r = 2 * r.unsigned_abs();
    let abs_den = denominator.unsigned_abs();

    if twice_abs_r < abs_den {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use proptest::prelude::*;

    fn date(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn spec_fixed(
        freq: Frequency,
        interval: u16,
        first_date: NaiveDate,
        policy: WeekendPolicy,
        end: EndSpec,
        amount: i64,
    ) -> RecurrenceSpec {
        RecurrenceSpec {
            frequency: freq,
            interval,
            by_monthday: Vec::new(),
            by_weekday: Vec::new(),
            weekend_policy: policy,
            first_date,
            end,
            amount: AmountMode::Fixed {
                amount_cents: amount,
            },
        }
    }

    #[test]
    fn daily_fixed_create() {
        let s = spec_fixed(
            Frequency::Daily,
            1,
            date("2024-01-01"),
            WeekendPolicy::Create,
            EndSpec::Never,
            100,
        );
        let result = generate_occurrences(&s, date("2024-01-01"), date("2024-01-05"));
        assert_eq!(result.len(), 5);
        assert_eq!(result[0].date, date("2024-01-01"));
        assert_eq!(result[4].date, date("2024-01-05"));
        assert!(result.iter().all(|o| o.amount_cents == 100));
    }

    #[test]
    fn daily_skip_weekend() {
        let s = spec_fixed(
            Frequency::Daily,
            1,
            date("2024-01-05"),
            WeekendPolicy::Skip,
            EndSpec::Never,
            50,
        );
        let result = generate_occurrences(&s, date("2024-01-05"), date("2024-01-12"));
        // Jan 5 (Fri), 8 (Mon), 9 (Tue), 10 (Wed), 11 (Thu), 12 (Fri)
        assert_eq!(result.len(), 6);
        assert_eq!(result[0].date, date("2024-01-05"));
        assert_eq!(result[1].date, date("2024-01-08"));
        assert_eq!(result[5].date, date("2024-01-12"));
    }

    #[test]
    fn weekly_mondays() {
        let s = spec_fixed(
            Frequency::Weekly,
            1,
            date("2024-01-01"),
            WeekendPolicy::Create,
            EndSpec::Never,
            100,
        );
        let result = generate_occurrences(&s, date("2024-01-01"), date("2024-01-29"));
        let dates: Vec<_> = result.iter().map(|o| o.date).collect();
        assert_eq!(
            dates,
            vec![
                date("2024-01-01"),
                date("2024-01-08"),
                date("2024-01-15"),
                date("2024-01-22"),
                date("2024-01-29")
            ]
        );
    }

    #[test]
    fn monthly_on_15th() {
        let mut s = spec_fixed(
            Frequency::Monthly,
            1,
            date("2024-01-15"),
            WeekendPolicy::Create,
            EndSpec::Never,
            200,
        );
        s.by_monthday = vec![15];
        let result = generate_occurrences(&s, date("2024-01-01"), date("2024-04-30"));
        let dates: Vec<_> = result.iter().map(|o| o.date).collect();
        assert_eq!(
            dates,
            vec![
                date("2024-01-15"),
                date("2024-02-15"),
                date("2024-03-15"),
                date("2024-04-15")
            ]
        );
    }

    #[test]
    fn yearly_anniversary() {
        let s = spec_fixed(
            Frequency::Yearly,
            1,
            date("2020-07-04"),
            WeekendPolicy::Create,
            EndSpec::Never,
            999,
        );
        let result = generate_occurrences(&s, date("2020-01-01"), date("2025-12-31"));
        let dates: Vec<_> = result.iter().map(|o| o.date).collect();
        assert_eq!(
            dates,
            vec![
                date("2020-07-04"),
                date("2021-07-04"),
                date("2022-07-04"),
                date("2023-07-04"),
                date("2024-07-04"),
                date("2025-07-04")
            ]
        );
    }

    #[test]
    fn weekend_shift_keeps_occurrences_at_the_window_edges() {
        // Monthly on the 27th, "friday" policy: 2025-12-27 is a Saturday, so the
        // first occurrence moves to Friday 2025-12-26 — before first_date. It must
        // still be generated when the window opens a couple of days earlier.
        let first = NaiveDate::from_ymd_opt(2025, 12, 27).unwrap();
        let spec = RecurrenceSpec {
            frequency: Frequency::Monthly,
            interval: 1,
            by_monthday: vec![27],
            by_weekday: vec![],
            weekend_policy: WeekendPolicy::Friday,
            first_date: first,
            end: EndSpec::Never,
            amount: AmountMode::Fixed { amount_cents: 100 },
        };
        let d = |y, m, dd| NaiveDate::from_ymd_opt(y, m, dd).unwrap();
        let occ = generate_occurrences(&spec, d(2025, 12, 25), d(2026, 1, 31));
        let dates: Vec<_> = occ.iter().map(|o| o.date).collect();
        assert_eq!(dates, vec![d(2025, 12, 26), d(2026, 1, 27)]);
        // Window ending on the shifted day: the raw Saturday (27th) is past `to`.
        let occ = generate_occurrences(&spec, d(2025, 12, 1), d(2025, 12, 26));
        assert_eq!(
            occ.iter().map(|o| o.date).collect::<Vec<_>>(),
            vec![d(2025, 12, 26)]
        );
    }

    #[test]
    fn last_day_of_month_rule_covers_short_months() {
        let d = |y, m, dd| NaiveDate::from_ymd_opt(y, m, dd).unwrap();
        let parts =
            crate::PatternParts::from_rrule("FREQ=MONTHLY;INTERVAL=1;BYMONTHDAY=-1").unwrap();
        assert_eq!(parts.by_monthday, vec![-1]);
        let mk = |first, by: Vec<i32>| RecurrenceSpec {
            frequency: Frequency::Monthly,
            interval: 1,
            by_monthday: by,
            by_weekday: vec![],
            weekend_policy: WeekendPolicy::Create,
            first_date: first,
            end: EndSpec::Never,
            amount: AmountMode::Fixed { amount_cents: 1 },
        };
        let dates = |spec: &RecurrenceSpec| {
            generate_occurrences(spec, d(2026, 1, 1), d(2026, 5, 31))
                .into_iter()
                .map(|o| o.date)
                .collect::<Vec<_>>()
        };
        // Starting mid-month or on the 31st: always the last day, leap years included.
        let expected = vec![
            d(2026, 1, 31),
            d(2026, 2, 28),
            d(2026, 3, 31),
            d(2026, 4, 30),
            d(2026, 5, 31),
        ];
        assert_eq!(dates(&mk(d(2026, 1, 15), vec![-1])), expected);
        assert_eq!(dates(&mk(d(2026, 1, 31), vec![-1])), expected);
        let leap =
            generate_occurrences(&mk(d(2028, 1, 31), vec![-1]), d(2028, 2, 1), d(2028, 2, 29));
        assert_eq!(
            leap.iter().map(|o| o.date).collect::<Vec<_>>(),
            vec![d(2028, 2, 29)]
        );
        // The plain "31st" rule skips the short months (the behaviour this rule fixes).
        assert_eq!(
            dates(&mk(d(2026, 1, 31), vec![31])),
            vec![d(2026, 1, 31), d(2026, 3, 31), d(2026, 5, 31)]
        );
    }

    #[test]
    fn holiday_calendar_drives_friday_monday_and_skip() {
        // Monthly on the 27th. 27 Dec 2026 is a Sunday; the Friday before is
        // Christmas, so with the Italian calendar "friday" lands on Thu 24 Dec.
        let d = |y, m, dd| NaiveDate::from_ymd_opt(y, m, dd).unwrap();
        let mk = |policy| RecurrenceSpec {
            frequency: Frequency::Monthly,
            interval: 1,
            by_monthday: vec![27],
            by_weekday: vec![],
            weekend_policy: policy,
            first_date: d(2026, 12, 1),
            end: EndSpec::Never,
            amount: AmountMode::Fixed { amount_cents: 1 },
        };
        let dates = |policy, cal: &HolidayCalendar| {
            generate_occurrences_in(&mk(policy), d(2026, 12, 1), d(2026, 12, 31), cal)
                .into_iter()
                .map(|o| o.date)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            dates(WeekendPolicy::Friday, &crate::calendar::fixtures::italy()),
            vec![d(2026, 12, 24)]
        );
        assert_eq!(
            dates(WeekendPolicy::Friday, &HolidayCalendar::weekends_only()),
            vec![d(2026, 12, 25)]
        );
        // Monday: Sunday 27 → Mon 28 (a holiday in the UK calendar: substitute Boxing Day → Tue 29).
        assert_eq!(
            dates(WeekendPolicy::Monday, &crate::calendar::fixtures::italy()),
            vec![d(2026, 12, 28)]
        );
        assert_eq!(
            dates(WeekendPolicy::Monday, &crate::calendar::fixtures::uk()),
            vec![d(2026, 12, 29)]
        );
        assert!(dates(WeekendPolicy::Skip, &crate::calendar::fixtures::italy()).is_empty());
        // A weekday holiday is skipped too: 25 Dec 2026 (Friday).
        let spec = RecurrenceSpec {
            by_monthday: vec![25],
            ..mk(WeekendPolicy::Skip)
        };
        assert!(generate_occurrences_in(
            &spec,
            d(2026, 12, 1),
            d(2026, 12, 31),
            &crate::calendar::fixtures::italy()
        )
        .is_empty());
        assert_eq!(
            generate_occurrences_in(
                &spec,
                d(2026, 12, 1),
                d(2026, 12, 31),
                &HolidayCalendar::weekends_only()
            )
            .len(),
            1
        );
    }

    #[test]
    fn weekend_friday_policy() {
        // 2024-01-06 is Saturday, 2024-01-07 is Sunday
        let s = spec_fixed(
            Frequency::Daily,
            1,
            date("2024-01-06"),
            WeekendPolicy::Friday,
            EndSpec::Never,
            100,
        );
        let result = generate_occurrences(&s, date("2024-01-06"), date("2024-01-08"));
        // Jan 6 (Sat) -> Fri Jan 5 (outside), Jan 7 (Sun) -> Fri Jan 5 (outside)
        // Jan 8 (Mon) stays as-is and is in range
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].date, date("2024-01-08"));
    }

    #[test]
    fn weekend_friday_policy_in_range() {
        // 2024-01-06 is Saturday
        let s = spec_fixed(
            Frequency::Daily,
            1,
            date("2024-01-06"),
            WeekendPolicy::Friday,
            EndSpec::Never,
            100,
        );
        // Widen the range so the shifted date is included
        let result = generate_occurrences(&s, date("2024-01-05"), date("2024-01-08"));
        // Jan 5 (Fri) from rrule, Jan 6 (Sat)->Jan 5 (Fri), Jan 7 (Sun)->Jan 5 (Fri), Jan 8 (Mon)
        // After dedup: Jan 5, Jan 8
        let dates: Vec<_> = result.iter().map(|o| o.date).collect();
        assert_eq!(dates, vec![date("2024-01-05"), date("2024-01-08")]);
    }

    #[test]
    fn weekend_monday_policy() {
        // 2024-01-06 is Saturday, 2024-01-07 is Sunday
        let s = spec_fixed(
            Frequency::Daily,
            1,
            date("2024-01-06"),
            WeekendPolicy::Monday,
            EndSpec::Never,
            100,
        );
        let result = generate_occurrences(&s, date("2024-01-06"), date("2024-01-10"));
        // Jan 6 (Sat)->Jan 8 (Mon), Jan 7 (Sun)->Jan 8 (Mon), Jan 8 (Mon), Jan 9 (Tue), Jan 10 (Wed)
        // After dedup: Jan 8, Jan 9, Jan 10
        let dates: Vec<_> = result.iter().map(|o| o.date).collect();
        assert_eq!(
            dates,
            vec![date("2024-01-08"), date("2024-01-09"), date("2024-01-10")]
        );
    }

    #[test]
    fn end_until() {
        let s = spec_fixed(
            Frequency::Daily,
            1,
            date("2024-01-01"),
            WeekendPolicy::Create,
            EndSpec::Until(date("2024-01-10")),
            100,
        );
        let result = generate_occurrences(&s, date("2024-01-01"), date("2024-01-31"));
        assert_eq!(result.len(), 10);
        assert_eq!(result.last().unwrap().date, date("2024-01-10"));
    }

    #[test]
    fn end_count() {
        let s = spec_fixed(
            Frequency::Daily,
            1,
            date("2024-01-01"),
            WeekendPolicy::Create,
            EndSpec::Count(5),
            100,
        );
        let result = generate_occurrences(&s, date("2024-01-01"), date("2024-01-31"));
        assert_eq!(result.len(), 5);
        assert_eq!(result.last().unwrap().date, date("2024-01-05"));
    }

    #[test]
    fn end_count_wider_horizon() {
        let s = spec_fixed(
            Frequency::Daily,
            1,
            date("2024-01-01"),
            WeekendPolicy::Create,
            EndSpec::Count(3),
            100,
        );
        // Horizon starts after the first occurrence
        let result = generate_occurrences(&s, date("2024-01-02"), date("2024-01-31"));
        // rrule generates Jan 1, 2, 3 (count=3). Horizon filters to Jan 2, 3.
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn from_after_to_returns_empty() {
        let s = spec_fixed(
            Frequency::Daily,
            1,
            date("2024-01-01"),
            WeekendPolicy::Create,
            EndSpec::Never,
            100,
        );
        let result = generate_occurrences(&s, date("2024-01-10"), date("2024-01-01"));
        assert!(result.is_empty());
    }

    #[test]
    fn avg_amount_basic() {
        let s = RecurrenceSpec {
            frequency: Frequency::Daily,
            interval: 1,
            by_monthday: Vec::new(),
            by_weekday: Vec::new(),
            weekend_policy: WeekendPolicy::Create,
            first_date: date("2024-01-04"),
            end: EndSpec::Never,
            amount: AmountMode::Avg {
                avg_n: 2,
                real_occurrences: vec![
                    RealOccurrence {
                        date: date("2024-01-01"),
                        amount_cents: 100,
                    },
                    RealOccurrence {
                        date: date("2024-01-02"),
                        amount_cents: 200,
                    },
                    RealOccurrence {
                        date: date("2024-01-03"),
                        amount_cents: 300,
                    },
                ],
                fallback_amount_cents: 0,
            },
        };
        let result = generate_occurrences(&s, date("2024-01-04"), date("2024-01-05"));
        // For Jan 4: last 2 real occ before Jan 4 are Jan 2 (200) and Jan 3 (300). Avg = 250.
        assert_eq!(result[0].amount_cents, 250);
        // For Jan 5: same (no new real occ between Jan 4 and Jan 5). Avg = 250.
        assert_eq!(result[1].amount_cents, 250);
    }

    #[test]
    fn avg_amount_no_prior() {
        let s = RecurrenceSpec {
            frequency: Frequency::Daily,
            interval: 1,
            by_monthday: Vec::new(),
            by_weekday: Vec::new(),
            weekend_policy: WeekendPolicy::Create,
            first_date: date("2024-01-01"),
            end: EndSpec::Never,
            amount: AmountMode::Avg {
                avg_n: 3,
                real_occurrences: vec![RealOccurrence {
                    date: date("2024-01-02"),
                    amount_cents: 100,
                }],
                fallback_amount_cents: 999,
            },
        };
        // Jan 1: no real occ before it → falls back to the rule's provided amount.
        // Jan 2: no real occ strictly before Jan 2 (the one IS on Jan 2) → same fallback.
        // Jan 3: 1 real occ before (Jan 2, 100). Avg = 100/1 = 100.
        let result = generate_occurrences(&s, date("2024-01-01"), date("2024-01-03"));
        assert_eq!(result[0].amount_cents, 999);
        assert_eq!(result[1].amount_cents, 999);
        assert_eq!(result[2].amount_cents, 100);
    }

    #[test]
    fn avg_amount_empty_real() {
        let s = RecurrenceSpec {
            frequency: Frequency::Daily,
            interval: 1,
            by_monthday: Vec::new(),
            by_weekday: Vec::new(),
            weekend_policy: WeekendPolicy::Create,
            first_date: date("2024-01-01"),
            end: EndSpec::Never,
            amount: AmountMode::Avg {
                avg_n: 3,
                real_occurrences: Vec::new(),
                fallback_amount_cents: 500,
            },
        };
        // No historical values at all → every occurrence uses the provided value.
        let result = generate_occurrences(&s, date("2024-01-01"), date("2024-01-03"));
        assert!(result.iter().all(|o| o.amount_cents == 500));
    }

    #[test]
    fn round_half_even_cases() {
        // 7/2 = 3.5 → 4 (round to even)
        assert_eq!(round_half_even(7, 2), 4);
        // 9/2 = 4.5 → 4 (round to even)
        assert_eq!(round_half_even(9, 2), 4);
        // 8/2 = 4 → 4
        assert_eq!(round_half_even(8, 2), 4);
        // 10/4 = 2.5 → 2 (round to even)
        assert_eq!(round_half_even(10, 4), 2);
        // 14/4 = 3.5 → 4 (round to even)
        assert_eq!(round_half_even(14, 4), 4);
        // -7/2 = -3.5 → -4 (round to even)
        assert_eq!(round_half_even(-7, 2), -4);
        // -9/2 = -4.5 → -4 (round to even)
        assert_eq!(round_half_even(-9, 2), -4);
        // 1/3 = 0.33 → 0
        assert_eq!(round_half_even(1, 3), 0);
        // 2/3 = 0.67 → 1
        assert_eq!(round_half_even(2, 3), 1);
    }

    #[test]
    fn by_weekday_first_monday() {
        let s = RecurrenceSpec {
            frequency: Frequency::Monthly,
            interval: 1,
            by_monthday: Vec::new(),
            by_weekday: vec![ByDay {
                ordinal: Some(1),
                weekday: Weekday::Mon,
            }],
            weekend_policy: WeekendPolicy::Create,
            first_date: date("2024-01-01"),
            end: EndSpec::Never,
            amount: AmountMode::Fixed { amount_cents: 100 },
        };
        let result = generate_occurrences(&s, date("2024-01-01"), date("2024-04-30"));
        let dates: Vec<_> = result.iter().map(|o| o.date).collect();
        // First Monday of each month:
        // Jan: Jan 1, Feb: Feb 5, Mar: Mar 4, Apr: Apr 1
        assert_eq!(
            dates,
            vec![
                date("2024-01-01"),
                date("2024-02-05"),
                date("2024-03-04"),
                date("2024-04-01")
            ]
        );
    }

    #[test]
    fn by_weekday_last_friday() {
        let s = RecurrenceSpec {
            frequency: Frequency::Monthly,
            interval: 1,
            by_monthday: Vec::new(),
            by_weekday: vec![ByDay {
                ordinal: Some(-1),
                weekday: Weekday::Fri,
            }],
            weekend_policy: WeekendPolicy::Create,
            first_date: date("2024-01-01"),
            end: EndSpec::Never,
            amount: AmountMode::Fixed { amount_cents: 100 },
        };
        let result = generate_occurrences(&s, date("2024-01-01"), date("2024-03-31"));
        let dates: Vec<_> = result.iter().map(|o| o.date).collect();
        // Last Friday: Jan 26, Feb 23, Mar 29
        assert_eq!(
            dates,
            vec![date("2024-01-26"), date("2024-02-23"), date("2024-03-29")]
        );
    }

    #[test]
    fn interval_biweekly() {
        let s = spec_fixed(
            Frequency::Weekly,
            2,
            date("2024-01-01"),
            WeekendPolicy::Create,
            EndSpec::Never,
            100,
        );
        let result = generate_occurrences(&s, date("2024-01-01"), date("2024-02-15"));
        let dates: Vec<_> = result.iter().map(|o| o.date).collect();
        assert_eq!(
            dates,
            vec![
                date("2024-01-01"),
                date("2024-01-15"),
                date("2024-01-29"),
                date("2024-02-12")
            ]
        );
    }

    #[test]
    fn leap_year_feb_29() {
        let s = RecurrenceSpec {
            frequency: Frequency::Yearly,
            interval: 1,
            by_monthday: Vec::new(),
            by_weekday: Vec::new(),
            weekend_policy: WeekendPolicy::Create,
            first_date: date("2024-02-29"),
            end: EndSpec::Never,
            amount: AmountMode::Fixed { amount_cents: 100 },
        };
        // 2024 is a leap year (has Feb 29), 2025 is not
        let result = generate_occurrences(&s, date("2024-01-01"), date("2028-12-31"));
        let dates: Vec<_> = result.iter().map(|o| o.date).collect();
        // Feb 29 exists in 2024 and 2028 (leap years), not in 2025, 2026, 2027
        assert_eq!(dates, vec![date("2024-02-29"), date("2028-02-29")]);
    }

    // ── proptest-based property tests ──────────────────────────────────────

    proptest! {
        #[test]
        fn prop_dates_in_range_and_unique(
            freq in prop::sample::select(&[
                Frequency::Yearly, Frequency::Monthly,
                Frequency::Weekly, Frequency::Daily,
            ]),
            interval in 1u16..4,
            policy in prop::sample::select(&[
                WeekendPolicy::Create, WeekendPolicy::Skip,
                WeekendPolicy::Friday, WeekendPolicy::Monday,
            ]),
            first_ymd in (2020i32..2025, 1u32..13, 1u32..29),
            from_offset in 0i64..365,
            horizon in 1u32..120,
            amount in -100000i64..100000,
        ) {
            let (fy, fm, fd) = first_ymd;
            let first_date = NaiveDate::from_ymd_opt(fy, fm, fd).unwrap();
            let from = first_date + Duration::days(from_offset);
            let to = from + Duration::days(horizon as i64);

            let s = RecurrenceSpec {
                frequency: freq,
                interval,
                by_monthday: Vec::new(),
                by_weekday: Vec::new(),
                weekend_policy: policy,
                first_date,
                end: EndSpec::Never,
                amount: AmountMode::Fixed { amount_cents: amount },
            };

            let result = generate_occurrences(&s, from, to);

            // All dates within [from, to]
            prop_assert!(result.iter().all(|o| o.date >= from && o.date <= to));
            // All dates unique
            let dates: Vec<_> = result.iter().map(|o| o.date).collect();
            let mut sorted = dates.clone();
            sorted.sort();
            sorted.dedup();
            prop_assert_eq!(dates.len(), sorted.len());
            // Sorted
            prop_assert!(dates.windows(2).all(|w| w[0] <= w[1]));
            // Fixed amount is uniform
            prop_assert!(result.iter().all(|o| o.amount_cents == amount));
        }

        #[test]
        fn prop_skip_no_weekend(
            first_ymd in (2020i32..2025, 1u32..13, 1u32..29),
            horizon in 1u32..90,
        ) {
            let (fy, fm, fd) = first_ymd;
            let first_date = NaiveDate::from_ymd_opt(fy, fm, fd).unwrap();
            let from = first_date;
            let to = from + Duration::days(horizon as i64);

            let s = RecurrenceSpec {
                frequency: Frequency::Daily,
                interval: 1,
                by_monthday: Vec::new(),
                by_weekday: Vec::new(),
                weekend_policy: WeekendPolicy::Skip,
                first_date,
                end: EndSpec::Never,
                amount: AmountMode::Fixed { amount_cents: 1 },
            };

            let result = generate_occurrences(&s, from, to);
            let no_weekend = !result.iter().any(|o| matches!(o.date.weekday(), Weekday::Sat | Weekday::Sun));
            prop_assert_eq!(no_weekend, true);
        }

        #[test]
        fn prop_friday_no_weekend(
            first_ymd in (2020i32..2025, 1u32..13, 1u32..29),
            horizon in 1u32..90,
        ) {
            let (fy, fm, fd) = first_ymd;
            let first_date = NaiveDate::from_ymd_opt(fy, fm, fd).unwrap();
            let from = first_date - Duration::days(2);
            let to = from + Duration::days(horizon as i64);

            let s = RecurrenceSpec {
                frequency: Frequency::Daily,
                interval: 1,
                by_monthday: Vec::new(),
                by_weekday: Vec::new(),
                weekend_policy: WeekendPolicy::Friday,
                first_date,
                end: EndSpec::Never,
                amount: AmountMode::Fixed { amount_cents: 1 },
            };

            let result = generate_occurrences(&s, from, to);
            let no_weekend = !result.iter().any(|o| matches!(o.date.weekday(), Weekday::Sat | Weekday::Sun));
            prop_assert_eq!(no_weekend, true);
        }

        #[test]
        fn prop_monday_no_weekend(
            first_ymd in (2020i32..2025, 1u32..13, 1u32..29),
            horizon in 1u32..90,
        ) {
            let (fy, fm, fd) = first_ymd;
            let first_date = NaiveDate::from_ymd_opt(fy, fm, fd).unwrap();
            let from = first_date;
            let to = from + Duration::days(horizon as i64);

            let s = RecurrenceSpec {
                frequency: Frequency::Daily,
                interval: 1,
                by_monthday: Vec::new(),
                by_weekday: Vec::new(),
                weekend_policy: WeekendPolicy::Monday,
                first_date,
                end: EndSpec::Never,
                amount: AmountMode::Fixed { amount_cents: 1 },
            };

            let result = generate_occurrences(&s, from, to);
            let no_weekend = !result.iter().any(|o| matches!(o.date.weekday(), Weekday::Sat | Weekday::Sun));
            prop_assert_eq!(no_weekend, true);
        }

        #[test]
        fn prop_count_respected(
            first_ymd in (2020i32..2025, 1u32..13, 1u32..29),
            count in 1u32..30,
            horizon in 1u32..365,
        ) {
            let (fy, fm, fd) = first_ymd;
            let first_date = NaiveDate::from_ymd_opt(fy, fm, fd).unwrap();
            let from = first_date;
            let to = from + Duration::days(horizon as i64);

            let s = RecurrenceSpec {
                frequency: Frequency::Daily,
                interval: 1,
                by_monthday: Vec::new(),
                by_weekday: Vec::new(),
                weekend_policy: WeekendPolicy::Create,
                first_date,
                end: EndSpec::Count(count),
                amount: AmountMode::Fixed { amount_cents: 1 },
            };

            let result = generate_occurrences(&s, from, to);
            prop_assert!(result.len() <= count as usize);
        }

        #[test]
        fn prop_until_respected(
            first_ymd in (2020i32..2025, 1u32..13, 1u32..29),
            until_offset in 1u32..60,
            horizon in 1u32..200,
        ) {
            let (fy, fm, fd) = first_ymd;
            let first_date = NaiveDate::from_ymd_opt(fy, fm, fd).unwrap();
            let until_date = first_date + Duration::days(until_offset as i64);
            let from = first_date;
            let to = from + Duration::days(horizon as i64);

            let s = RecurrenceSpec {
                frequency: Frequency::Daily,
                interval: 1,
                by_monthday: Vec::new(),
                by_weekday: Vec::new(),
                weekend_policy: WeekendPolicy::Create,
                first_date,
                end: EndSpec::Until(until_date),
                amount: AmountMode::Fixed { amount_cents: 1 },
            };

            let result = generate_occurrences(&s, from, to);
            prop_assert!(result.iter().all(|o| o.date <= until_date));
        }

        #[test]
        fn prop_monthly_31_skips_short_months(
            year in 2020i32..2030,
        ) {
            // Monthly on the 31st: months without a 31st are skipped
            let first_date = NaiveDate::from_ymd_opt(year, 1, 31).unwrap();
            let from = first_date;
            let to = from + Duration::days(400);

            let s = RecurrenceSpec {
                frequency: Frequency::Monthly,
                interval: 1,
                by_monthday: vec![31],
                by_weekday: Vec::new(),
                weekend_policy: WeekendPolicy::Create,
                first_date,
                end: EndSpec::Never,
                amount: AmountMode::Fixed { amount_cents: 1 },
            };

            let result = generate_occurrences(&s, from, to);
            // Every result must be on the 31st
            prop_assert!(result.iter().all(|o| o.date.day() == 31));
        }
    }
}
