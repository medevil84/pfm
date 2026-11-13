//! The account "spending rule" entity (canoni, imposte di bollo, commissioni).
//!
//! A persisted rule bound to a single account. It carries the parameters the
//! pure [`crate::fee_engine`] consumes (via [`AccountRule::fee_rule`]) plus the
//! bookkeeping fields (id, account, category, name, active, timestamps). The
//! rows the engine materializes for a rule live in `transactions` with
//! `rule_id` set and `computed = 1`; this entity stores only the rule itself.
//!
//! This module is intentionally free of I/O, serialization, or framework
//! dependencies: it only depends on `chrono` and the pure fee engine.

use crate::fee_engine::{Cadence, CalcType, CondBase, CondOp, FeeRule};

/// A persisted spending rule for an account (the full entity, as stored in
/// `account_rules`).
///
/// `value` is interpreted by `calc_type`: cents for [`CalcType::Fixed`], basis
/// points for the percentage types. When `is_annual` is set, `value` is the
/// annual amount distributed over the year's charged deadlines.
#[derive(Debug, Clone)]
pub struct AccountRule {
    /// Opaque identifier (UUID string, application-generated).
    pub id: String,
    /// The account this rule is recorded on.
    pub account_id: String,
    /// Short human-readable name (e.g. "Imposta di bollo").
    pub name: String,
    /// The category the generated fee rows are classified under.
    pub category_id: String,
    /// How the fee amount is computed.
    pub calc_type: CalcType,
    /// Amount: cents (fixed) or basis points (percentage).
    pub value: i64,
    /// Whether `value` is an annual amount to distribute over the deadlines.
    pub is_annual: bool,
    /// Whether a fixed annual amount is charged pro-rata on each period's
    /// actual days and debited in arrears (stamp-duty style).
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
    /// Comparison operator (ignored when the base is `Always`).
    pub cond_op: Option<CondOp>,
    /// Threshold: cents for balance/flow bases, a count for the `TxCount` base.
    pub cond_threshold: Option<i64>,
    /// Whether the rule is active (vs. paused/archived).
    pub active: bool,
    /// Creation timestamp, RFC-3339 UTC.
    pub created_at: String,
    /// Last update timestamp, RFC-3339 UTC.
    pub updated_at: String,
}

impl AccountRule {
    /// Build the pure [`FeeRule`] the engine evaluates. All bookkeeping fields
    /// (id, account, category, name, active, timestamps) are dropped.
    pub fn fee_rule(&self) -> FeeRule {
        FeeRule {
            calc_type: self.calc_type,
            value: self.value,
            is_annual: self.is_annual,
            prorata_days: self.prorata_days,
            annual_cap_cents: self.annual_cap_cents,
            cadence: self.cadence,
            charge_day: self.charge_day,
            charge_month: self.charge_month,
            cond_base: self.cond_base,
            cond_op: self.cond_op,
            cond_threshold: self.cond_threshold,
        }
    }
}
