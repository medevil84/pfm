//! PFM domain types (pure Rust, no I/O or serialization crates):
//! authentication & user management, plus accounts and categories.

pub mod account;
pub mod account_rule;
pub mod audit;
pub mod balance_engine;
pub mod budget_engine;
pub mod calendar;
pub mod card_engine;
pub mod category;
pub mod credentials;
pub mod deposit_engine;
pub mod export;
pub mod fee_engine;
pub mod import;
pub mod password;
pub mod permission_engine;
pub mod prefs;
pub mod recurrence;
pub mod recurrence_engine;
pub mod role;
pub mod transaction;
pub mod user;

pub use account::{Account, AccountKind};
pub use account_rule::AccountRule;
pub use audit::{AuditAction, AuditEvent, AuditRecord};
pub use balance_engine::{build_register, virtual_balances, VirtualBalances};
pub use budget_engine::{budget_progress, prev_period_range, BudgetProgress};
pub use calendar::{HolidayCalendar, HolidayRule, Observance, RuleKind, RuleParts};
pub use card_engine::{
    all_due_cycles, charge_amount, cycle_bounds, cycle_for_date, effective_close,
    next_business_day_in, Cycle,
};
pub use category::{Category, CategoryKind};
pub use credentials::Credentials;
pub use deposit_engine::{compute_interest, DailyBalance, InterestResult};
pub use export::{
    cents_to_decimal, date_to_dmy, escape_csv_field, format_export_csv, ExportRow, EXPORT_HEADER,
};
pub use fee_engine::{
    cycle_fee_amount, fee_horizon, fee_horizon_in, generate_fees, generate_fees_in, Cadence,
    CalcType, CondBase, CondOp, FeePeriod, FeeRow, FeeRule, PeriodMetrics,
};
pub use import::{
    detect_columns, parse_amount_str, parse_csv, parse_date_str, ColumnRole, DateFormat,
    DecimalSep, DedupKey, ImportProfile, ImportRow,
};
pub use password::ChangePassword;
pub use permission_engine::{can_see, can_write, ShareEntry, ShareLevel};
pub use prefs::{is_valid_amount_color, UserPrefs};
pub use recurrence::{
    EndType, PatternParts, Recurrence, RecurrenceMode, RecurrenceOccurrence, RecurrenceOverride,
};
pub use recurrence_engine::{
    generate_occurrences, generate_occurrences_in, AmountMode, ByDay, EndSpec, Frequency,
    Occurrence, RealOccurrence, RecurrenceSpec, WeekendPolicy, SHIFT_MARGIN_DAYS,
};
pub use role::Role;
pub use transaction::{
    RegisterDay, RegisterResult, RegisterSummary, Transaction, TransactionRow, Transfer,
};
pub use user::{AuthUser, User};
