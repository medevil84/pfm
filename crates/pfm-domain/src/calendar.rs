//! Business-day calendars: which days a bank does not post.
//!
//! A [`HolidayCalendar`] is **data**: an ordered list of [`HolidayRule`]s (fixed
//! dates, Easter-relative dates, "nth weekday of a month", single dates) that an
//! administrator edits; this module only knows how to *evaluate* those rules.
//! Saturday and Sunday are always non-business days.
//!
//! Pure date logic: no I/O, no dependencies beyond `chrono`.

use std::collections::BTreeMap;

use chrono::{Datelike, Duration, NaiveDate, Weekday};

/// What happens when a fixed-date holiday falls on a weekend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observance {
    /// Stays on its date (it is then invisible: weekends are never business days).
    None,
    /// Saturday → previous Friday, Sunday → next Monday (US federal rule).
    NearestWeekday,
    /// Moves to the next weekday not already taken by a rule listed *before* it
    /// (UK substitute days).
    NextFreeWeekday,
}

/// How a holiday date is computed for a given year.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleKind {
    /// Same month/day every year.
    Fixed {
        month: u32,
        day: u32,
        observance: Observance,
    },
    /// Easter Sunday plus `offset` days (Easter Monday = 1, Good Friday = -2).
    Easter { offset: i64 },
    /// The `nth` `weekday` of `month`; `nth` is 1..=5, or -1 for the last one.
    NthWeekday {
        month: u32,
        weekday: Weekday,
        nth: i32,
    },
    /// One specific date (extraordinary closures, local holidays for one year).
    Date(NaiveDate),
}

/// One holiday rule with a human label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HolidayRule {
    pub label: String,
    pub kind: RuleKind,
}

/// Flat, storage/transport form of a [`HolidayRule`] (database columns and API).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleParts {
    pub label: String,
    /// `fixed`, `easter`, `nth_weekday` or `date`.
    pub kind: String,
    pub month: Option<i32>,
    pub day: Option<i32>,
    pub offset_days: Option<i32>,
    /// 1 = Monday … 7 = Sunday.
    pub weekday: Option<i32>,
    pub nth: Option<i32>,
    /// `none`, `nearest` or `next_free` (fixed rules only; absent = `none`).
    pub observance: Option<String>,
    /// `YYYY-MM-DD` (date rules only).
    pub date: Option<String>,
}

impl HolidayRule {
    /// Build a rule from its flat form, validating every field.
    pub fn from_parts(p: &RuleParts) -> Result<HolidayRule, String> {
        let label = p.label.trim().to_string();
        if label.is_empty() {
            return Err("rule label must not be empty".into());
        }
        if label.chars().count() > 100 {
            return Err("rule label is longer than 100 characters".into());
        }
        let req = |v: Option<i32>, name: &str| {
            v.ok_or_else(|| format!("{name} is required for '{}'", p.kind))
        };
        let kind = match p.kind.as_str() {
            "fixed" => {
                let month = req(p.month, "month")?;
                let day = req(p.day, "day")?;
                // 2024 is a leap year: accepts 29 Feb, rejects 31 Apr, 0, 32…
                if !(1..=12).contains(&month)
                    || NaiveDate::from_ymd_opt(2024, month as u32, day.max(0) as u32).is_none()
                {
                    return Err(format!("invalid date {day}/{month}"));
                }
                let observance = match p.observance.as_deref() {
                    None | Some("none") | Some("") => Observance::None,
                    Some("nearest") => Observance::NearestWeekday,
                    Some("next_free") => Observance::NextFreeWeekday,
                    Some(o) => return Err(format!("unknown observance '{o}'")),
                };
                RuleKind::Fixed {
                    month: month as u32,
                    day: day as u32,
                    observance,
                }
            }
            "easter" => {
                let offset = req(p.offset_days, "offset_days")?;
                if !(-120..=120).contains(&offset) {
                    return Err("offset_days must be between -120 and 120".into());
                }
                RuleKind::Easter {
                    offset: offset as i64,
                }
            }
            "nth_weekday" => {
                let month = req(p.month, "month")?;
                let wd = req(p.weekday, "weekday")?;
                let nth = req(p.nth, "nth")?;
                if !(1..=12).contains(&month) {
                    return Err("month must be 1..12".into());
                }
                if !(1..=7).contains(&wd) {
                    return Err("weekday must be 1 (Mon) .. 7 (Sun)".into());
                }
                if !(1..=5).contains(&nth) && nth != -1 {
                    return Err("nth must be 1..5 or -1 (last)".into());
                }
                RuleKind::NthWeekday {
                    month: month as u32,
                    weekday: Weekday::try_from((wd - 1) as u8)
                        .map_err(|_| "bad weekday".to_string())?,
                    nth,
                }
            }
            "date" => {
                let s = p.date.as_deref().ok_or("date is required for 'date'")?;
                RuleKind::Date(
                    NaiveDate::parse_from_str(s, "%Y-%m-%d")
                        .map_err(|_| format!("invalid date '{s}'"))?,
                )
            }
            other => return Err(format!("unknown rule kind '{other}'")),
        };
        Ok(HolidayRule { label, kind })
    }

    /// The flat form of this rule.
    pub fn to_parts(&self) -> RuleParts {
        let mut p = RuleParts {
            label: self.label.clone(),
            ..Default::default()
        };
        match self.kind {
            RuleKind::Fixed {
                month,
                day,
                observance,
            } => {
                p.kind = "fixed".into();
                p.month = Some(month as i32);
                p.day = Some(day as i32);
                p.observance = Some(
                    match observance {
                        Observance::None => "none",
                        Observance::NearestWeekday => "nearest",
                        Observance::NextFreeWeekday => "next_free",
                    }
                    .into(),
                );
            }
            RuleKind::Easter { offset } => {
                p.kind = "easter".into();
                p.offset_days = Some(offset as i32);
            }
            RuleKind::NthWeekday {
                month,
                weekday,
                nth,
            } => {
                p.kind = "nth_weekday".into();
                p.month = Some(month as i32);
                p.weekday = Some(weekday.number_from_monday() as i32);
                p.nth = Some(nth);
            }
            RuleKind::Date(d) => {
                p.kind = "date".into();
                p.date = Some(d.format("%Y-%m-%d").to_string());
            }
        }
        p
    }
}

/// An ordered set of holiday rules. The default has no rules: weekends only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HolidayCalendar {
    pub rules: Vec<HolidayRule>,
}

impl HolidayCalendar {
    /// A calendar with no holidays: only Saturday and Sunday are off.
    pub fn weekends_only() -> Self {
        Self::default()
    }

    pub fn new(rules: Vec<HolidayRule>) -> Self {
        Self { rules }
    }

    /// Public holidays of `year` with their labels, sorted by date. May include a
    /// date of an adjacent year when observance moves it there.
    pub fn holidays_of_year(&self, year: i32) -> Vec<(NaiveDate, String)> {
        let easter = easter_sunday(year);
        // date → label; `taken` semantics for NextFreeWeekday read this map.
        let mut found: BTreeMap<NaiveDate, String> = BTreeMap::new();
        for rule in &self.rules {
            let date = match rule.kind {
                RuleKind::Fixed {
                    month,
                    day,
                    observance,
                } => {
                    let Some(mut d) = NaiveDate::from_ymd_opt(year, month, day) else {
                        continue;
                    };
                    match observance {
                        Observance::None => {}
                        Observance::NearestWeekday => {
                            d = match d.weekday() {
                                Weekday::Sat => d - Duration::days(1),
                                Weekday::Sun => d + Duration::days(1),
                                _ => d,
                            };
                        }
                        Observance::NextFreeWeekday => {
                            while matches!(d.weekday(), Weekday::Sat | Weekday::Sun)
                                || found.contains_key(&d)
                            {
                                d += Duration::days(1);
                            }
                        }
                    }
                    d
                }
                RuleKind::Easter { offset } => easter + Duration::days(offset),
                RuleKind::NthWeekday {
                    month,
                    weekday,
                    nth,
                } => {
                    let Some(d) = nth_weekday(year, month, weekday, nth) else {
                        continue;
                    };
                    d
                }
                RuleKind::Date(d) => {
                    if d.year() != year {
                        continue;
                    }
                    d
                }
            };
            found.entry(date).or_insert_with(|| rule.label.clone());
        }
        found.into_iter().collect()
    }

    /// Whether `date` is a public holiday (weekends are not holidays here).
    pub fn is_holiday(&self, date: NaiveDate) -> bool {
        if self.rules.is_empty() {
            return false;
        }
        let in_year = |y: i32| self.holidays_of_year(y).iter().any(|(d, _)| *d == date);
        // A holiday observed across the year boundary (e.g. 1 Jan on a Saturday
        // observed on 31 Dec of the previous year) is found by the next year.
        in_year(date.year()) || (date.month() == 12 && date.day() == 31 && in_year(date.year() + 1))
    }

    /// Weekend or public holiday.
    pub fn is_non_business_day(&self, date: NaiveDate) -> bool {
        matches!(date.weekday(), Weekday::Sat | Weekday::Sun) || self.is_holiday(date)
    }

    /// `date` itself if it is a business day, else the next business day.
    pub fn next_business_day(&self, mut date: NaiveDate) -> NaiveDate {
        // Longest realistic chain: Christmas + Boxing Day + weekend ≈ 5 days.
        for _ in 0..14 {
            if !self.is_non_business_day(date) {
                break;
            }
            date += Duration::days(1);
        }
        date
    }

    /// `date` itself if it is a business day, else the previous business day.
    pub fn previous_business_day(&self, mut date: NaiveDate) -> NaiveDate {
        for _ in 0..14 {
            if !self.is_non_business_day(date) {
                break;
            }
            date -= Duration::days(1);
        }
        date
    }
}

/// The `nth` (1-based, or -1 = last) `weekday` of `month`; `None` if it does not exist.
fn nth_weekday(year: i32, month: u32, weekday: Weekday, nth: i32) -> Option<NaiveDate> {
    if nth == -1 {
        let (ny, nm) = if month == 12 {
            (year + 1, 1)
        } else {
            (year, month + 1)
        };
        let mut d = NaiveDate::from_ymd_opt(ny, nm, 1)? - Duration::days(1);
        while d.weekday() != weekday {
            d -= Duration::days(1);
        }
        return Some(d);
    }
    let first = NaiveDate::from_ymd_opt(year, month, 1)?;
    let offset = (7 + weekday.num_days_from_monday() as i64
        - first.weekday().num_days_from_monday() as i64)
        % 7;
    let d = first + Duration::days(offset + 7 * (nth as i64 - 1));
    (d.month() == month).then_some(d)
}

/// Gregorian Easter Sunday (Meeus/Jones/Butcher algorithm).
pub fn easter_sunday(year: i32) -> NaiveDate {
    let a = year % 19;
    let b = year / 100;
    let c = year % 100;
    let d = b / 4;
    let e = b % 4;
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let i = c / 4;
    let k = c % 4;
    let l = (32 + 2 * e + 2 * i - h - k) % 7;
    let m = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * m + 114) / 31;
    let day = ((h + l - 7 * m + 114) % 31) + 1;
    NaiveDate::from_ymd_opt(year, month as u32, day as u32).expect("valid Easter date")
}

/// Calendars used as test fixtures by this crate's tests. The real calendars live
/// in the database; these only exercise the rule evaluator.
#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    pub fn fixed(label: &str, month: u32, day: u32, observance: Observance) -> HolidayRule {
        HolidayRule {
            label: label.into(),
            kind: RuleKind::Fixed {
                month,
                day,
                observance,
            },
        }
    }
    pub fn easter(label: &str, offset: i64) -> HolidayRule {
        HolidayRule {
            label: label.into(),
            kind: RuleKind::Easter { offset },
        }
    }
    pub fn nth(label: &str, month: u32, weekday: Weekday, nth: i32) -> HolidayRule {
        HolidayRule {
            label: label.into(),
            kind: RuleKind::NthWeekday {
                month,
                weekday,
                nth,
            },
        }
    }

    pub fn italy() -> HolidayCalendar {
        let n = Observance::None;
        let mut rules: Vec<_> = [
            (1, 1),
            (1, 6),
            (4, 25),
            (5, 1),
            (6, 2),
            (8, 15),
            (11, 1),
            (12, 8),
            (12, 25),
            (12, 26),
        ]
        .into_iter()
        .map(|(m, d)| fixed("fixed", m, d, n))
        .collect();
        rules.push(easter("Pasquetta", 1));
        HolidayCalendar::new(rules)
    }

    pub fn uk() -> HolidayCalendar {
        let nf = Observance::NextFreeWeekday;
        HolidayCalendar::new(vec![
            fixed("New Year", 1, 1, nf),
            fixed("Christmas", 12, 25, nf),
            fixed("Boxing Day", 12, 26, nf),
            easter("Good Friday", -2),
            easter("Easter Monday", 1),
            nth("Early May", 5, Weekday::Mon, 1),
            nth("Spring", 5, Weekday::Mon, -1),
            nth("Summer", 8, Weekday::Mon, -1),
        ])
    }

    pub fn us() -> HolidayCalendar {
        let ne = Observance::NearestWeekday;
        HolidayCalendar::new(vec![
            fixed("New Year", 1, 1, ne),
            fixed("Juneteenth", 6, 19, ne),
            fixed("Independence", 7, 4, ne),
            fixed("Veterans", 11, 11, ne),
            fixed("Christmas", 12, 25, ne),
            nth("MLK", 1, Weekday::Mon, 3),
            nth("Thanksgiving", 11, Weekday::Thu, 4),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn easter_known_dates() {
        assert_eq!(easter_sunday(2025), d("2025-04-20"));
        assert_eq!(easter_sunday(2026), d("2026-04-05"));
        assert_eq!(easter_sunday(2027), d("2027-03-28"));
    }

    #[test]
    fn empty_calendar_is_weekends_only() {
        let c = HolidayCalendar::weekends_only();
        assert!(!c.is_holiday(d("2026-12-25")));
        assert_eq!(c.previous_business_day(d("2026-12-27")), d("2026-12-25"));
    }

    #[test]
    fn italy_business_days() {
        let it = italy();
        assert!(it.is_holiday(d("2026-04-06"))); // Pasquetta
        assert!(!it.is_holiday(d("2026-10-12")));
        assert_eq!(it.previous_business_day(d("2026-12-27")), d("2026-12-24"));
        assert_eq!(it.next_business_day(d("2026-12-25")), d("2026-12-28"));
    }

    #[test]
    fn uk_substitute_days_and_nth_weekday() {
        let gb = uk();
        // 2026: Christmas is a Friday, Boxing Day a Saturday → substitute Mon 28.
        assert!(gb.is_holiday(d("2026-12-25")));
        assert!(gb.is_holiday(d("2026-12-28")));
        assert!(!gb.is_holiday(d("2026-12-26")));
        // 2027: 25 Dec Sat → Mon 27; 26 Dec Sun → Mon is taken → Tue 28.
        assert!(gb.is_holiday(d("2027-12-27")) && gb.is_holiday(d("2027-12-28")));
        assert!(
            gb.is_holiday(d("2026-05-04"))
                && gb.is_holiday(d("2026-05-25"))
                && gb.is_holiday(d("2026-08-31"))
        );
    }

    #[test]
    fn us_observance_across_year_boundary() {
        let us = us();
        assert!(us.is_holiday(d("2026-07-03"))); // 4 July is a Saturday → Friday
        assert!(us.is_holiday(d("2026-11-26"))); // 4th Thursday of November
        assert!(us.is_holiday(d("2026-01-19"))); // 3rd Monday of January
        assert!(us.is_holiday(d("2027-12-31"))); // 1 Jan 2028 is a Saturday
    }

    #[test]
    fn single_date_rule_only_matches_its_year() {
        let c = HolidayCalendar::new(vec![HolidayRule {
            label: "closure".into(),
            kind: RuleKind::Date(d("2026-03-10")),
        }]);
        assert!(c.is_holiday(d("2026-03-10")));
        assert!(!c.is_holiday(d("2027-03-10")));
    }

    #[test]
    fn rule_parts_round_trip_and_validation() {
        for r in [
            fixed("a", 12, 25, Observance::NextFreeWeekday),
            easter("b", -2),
            nth("c", 5, Weekday::Mon, -1),
            HolidayRule {
                label: "d".into(),
                kind: RuleKind::Date(d("2026-03-10")),
            },
        ] {
            assert_eq!(HolidayRule::from_parts(&r.to_parts()).unwrap(), r);
        }
        let bad = |p: RuleParts| HolidayRule::from_parts(&p).is_err();
        let base = RuleParts {
            label: "x".into(),
            kind: "fixed".into(),
            month: Some(4),
            day: Some(31),
            ..Default::default()
        };
        assert!(bad(base.clone())); // 31 April
        assert!(bad(RuleParts {
            label: " ".into(),
            ..base.clone()
        }));
        assert!(bad(RuleParts {
            kind: "weird".into(),
            ..base.clone()
        }));
        assert!(bad(RuleParts {
            kind: "nth_weekday".into(),
            month: Some(5),
            weekday: Some(8),
            nth: Some(1),
            ..base.clone()
        }));
        assert!(!bad(RuleParts {
            day: Some(30),
            ..base
        }));
    }
}
