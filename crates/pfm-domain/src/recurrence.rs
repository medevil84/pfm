//! Recurrence rule entity, per-date overrides, and the value objects used to
//! bridge a stored rule to the pure [`recurrence_engine`] (pattern parsing,
//! end/amount spec construction).
//!
//! This module is intentionally free of I/O, serialization, or framework
//! dependencies: it only depends on `chrono` and `rrule`.

use chrono::NaiveDate;

use crate::category::CategoryKind;
use crate::recurrence_engine::{
    AmountMode, ByDay, EndSpec, Frequency, RealOccurrence, WeekendPolicy,
};

/// How the amount for each occurrence of a rule is determined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecurrenceMode {
    /// Every occurrence carries the fixed [`Recurrence::amount_cents`].
    Fixed,
    /// Each occurrence's amount is the average of the last `avg_n` real
    /// (historical / confirmed) occurrences that strictly precede it.
    Avg,
}

impl RecurrenceMode {
    /// Stable string form used on the wire and in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            RecurrenceMode::Fixed => "fixed",
            RecurrenceMode::Avg => "avg",
        }
    }

    /// Parse a mode from its string form. `None` for unknown values.
    pub fn parse(s: &str) -> Option<RecurrenceMode> {
        Some(match s {
            "fixed" => RecurrenceMode::Fixed,
            "avg" => RecurrenceMode::Avg,
            _ => return None,
        })
    }
}

impl std::fmt::Display for RecurrenceMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How a recurrence ends. The concrete end data lives in the `end_date` /
/// `end_count` fields on [`Recurrence`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndType {
    /// The recurrence never ends.
    Never,
    /// Ends on or before [`Recurrence::end_date`] (inclusive).
    Until,
    /// Ends after [`Recurrence::end_count`] total occurrences.
    Count,
}

impl EndType {
    /// Stable string form used on the wire and in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            EndType::Never => "never",
            EndType::Until => "until",
            EndType::Count => "count",
        }
    }

    /// Parse an end type from its string form. `None` for unknown values.
    pub fn parse(s: &str) -> Option<EndType> {
        Some(match s {
            "never" => EndType::Never,
            "until" => EndType::Until,
            "count" => EndType::Count,
            _ => return None,
        })
    }
}

impl std::fmt::Display for EndType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A persisted recurrence rule (the full entity, as stored in `recurrences`).
///
/// `amount_cents` carries the sign (negative = out, positive = in), consistent
/// with transaction cents. `pattern` is an RFC-5545 RRULE string (e.g.
/// `FREQ=MONTHLY;BYMONTHDAY=15`); the structural parts are parsed on demand by
/// [`PatternParts::from_rrule`] when occurrences are generated.
#[derive(Debug, Clone)]
pub struct Recurrence {
    /// Opaque identifier (UUID string, application-generated).
    pub id: String,
    /// The user who owns this rule.
    pub owner_id: String,
    /// Short description (e.g. "Stipendio").
    pub description: String,
    /// The category this rule is classified under.
    pub category_id: String,
    /// Direction: out (expense) or in (income). Immutable after creation.
    pub kind: CategoryKind,
    /// Base amount in cents (signed). For `avg` rules this is the fallback
    /// used before any real occurrence exists.
    pub amount_cents: i64,
    /// The account the occurrences are recorded on.
    pub account_id: String,
    /// RFC-5545 RRULE string (e.g. `FREQ=MONTHLY;BYMONTHDAY=15`).
    pub pattern: String,
    /// How to treat occurrences that land on a weekend.
    pub weekend: WeekendPolicy,
    /// The first date of the recurrence (DTSTART).
    pub first_date: NaiveDate,
    /// How the recurrence ends.
    pub end_type: EndType,
    /// End date (inclusive) when `end_type` is [`EndType::Until`].
    pub end_date: Option<NaiveDate>,
    /// Total occurrence count when `end_type` is [`EndType::Count`].
    pub end_count: Option<i64>,
    /// How the amount per occurrence is computed.
    pub mode: RecurrenceMode,
    /// Number of trailing real occurrences used for the average (avg mode).
    pub avg_n: i64,
    /// Whether the rule is active (vs. paused/archived).
    pub active: bool,
    /// Free-form note.
    pub note: String,
    /// When set, each occurrence also fires a paired transaction on this account
    /// (opposite sign), linking source and destination like a one-time transfer.
    pub linked_account_id: Option<String>,
    /// Category applied to the linked-account transaction leg of a transfer.
    /// Falls back to `category_id` when absent.
    pub linked_category_id: Option<String>,
    /// Creation timestamp, RFC-3339 UTC.
    pub created_at: String,
    /// Last update timestamp, RFC-3339 UTC.
    pub updated_at: String,
}

impl Recurrence {
    /// Build the engine [`EndSpec`] from the stored end fields.
    pub fn end_spec(&self) -> EndSpec {
        match self.end_type {
            EndType::Never => EndSpec::Never,
            EndType::Until => EndSpec::Until(self.end_date.unwrap_or(self.first_date)),
            EndType::Count => EndSpec::Count(self.end_count.unwrap_or(0) as u32),
        }
    }

    /// Build the engine [`AmountMode`]. The caller supplies the real
    /// occurrences (only consumed by the `avg` mode).
    pub fn amount_mode(&self, real_occurrences: Vec<RealOccurrence>) -> AmountMode {
        match self.mode {
            RecurrenceMode::Fixed => AmountMode::Fixed {
                amount_cents: self.amount_cents,
            },
            RecurrenceMode::Avg => AmountMode::Avg {
                avg_n: self.avg_n.max(1) as u32,
                real_occurrences,
                fallback_amount_cents: self.amount_cents,
            },
        }
    }
}

/// A persisted per-date override of a recurrence rule's estimated amount, as
/// stored in `recurrence_overrides`. When a user confirms a generated row with
/// a real amount different from the estimate, the confirmed value is persisted
/// here; it is never regenerated by the engine.
#[derive(Debug, Clone)]
pub struct RecurrenceOverride {
    /// Opaque identifier (UUID string, application-generated).
    pub id: String,
    /// The recurrence rule this override belongs to.
    pub rec_id: String,
    /// The occurrence date being overridden.
    pub date: NaiveDate,
    /// The confirmed real amount in cents (signed).
    pub amount_cents: i64,
    /// Optional description for this specific occurrence.
    pub description: Option<String>,
    /// Creation timestamp, RFC-3339 UTC.
    pub created_at: String,
}

/// The structural parts parsed from an RFC-5545 RRULE `pattern` string.
///
/// Only the parts the engine consumes are retained: frequency, interval,
/// `BYMONTHDAY` and `BYDAY`. End / start / amount are stored as separate
/// columns on [`Recurrence`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternParts {
    /// The base frequency.
    pub frequency: Frequency,
    /// The interval between occurrences.
    pub interval: u16,
    /// Days of the month: positive from the start (`15`), negative from the end
    /// (`-1` = last day of the month).
    pub by_monthday: Vec<i32>,
    /// Day-of-week selectors (e.g. first Monday, last Friday).
    pub by_weekday: Vec<ByDay>,
}

impl PatternParts {
    /// Parse an RFC-5545 RRULE string into its structural parts.
    ///
    /// Returns a descriptive error string when the pattern is malformed or uses
    /// a frequency / ordinal the engine does not support.
    pub fn from_rrule(pattern: &str) -> Result<Self, String> {
        use rrule::{NWeekday, RRule, Unvalidated};

        let rrule: RRule<Unvalidated> = pattern
            .parse()
            .map_err(|e: rrule::RRuleError| format!("invalid rrule pattern: {e}"))?;

        let frequency = match rrule.get_freq() {
            rrule::Frequency::Yearly => Frequency::Yearly,
            rrule::Frequency::Monthly => Frequency::Monthly,
            rrule::Frequency::Weekly => Frequency::Weekly,
            rrule::Frequency::Daily => Frequency::Daily,
            other => return Err(format!("unsupported frequency: {other}")),
        };

        let by_monthday = rrule
            .get_by_month_day()
            .iter()
            .map(|d| i32::from(*d))
            .filter(|d| *d != 0)
            .collect();

        let mut by_weekday = Vec::with_capacity(rrule.get_by_weekday().len());
        for nw in rrule.get_by_weekday() {
            let ordinal = match nw {
                NWeekday::Every(_) => None,
                NWeekday::Nth(n, _) => {
                    let ord = i8::try_from(*n)
                        .map_err(|_| format!("weekday ordinal out of range: {n}"))?;
                    Some(ord)
                }
            };
            let weekday = match nw {
                NWeekday::Every(w) => *w,
                NWeekday::Nth(_, w) => *w,
            };
            by_weekday.push(ByDay { ordinal, weekday });
        }

        Ok(Self {
            frequency,
            interval: rrule.get_interval(),
            by_monthday,
            by_weekday,
        })
    }
}

/// A generated occurrence as returned by the recurrence service: the engine's
/// date/amount, possibly replaced by a persisted override, with metadata so the
/// client can render confirmed vs. estimated rows.
#[derive(Debug, Clone)]
pub struct RecurrenceOccurrence {
    /// The date of the occurrence.
    pub date: NaiveDate,
    /// The amount in cents for this occurrence (confirmed when `overridden`).
    pub amount_cents: i64,
    /// Description, taken from the override when present, else the rule's.
    pub description: Option<String>,
    /// Whether a persisted override fixed this occurrence's value.
    pub overridden: bool,
}
