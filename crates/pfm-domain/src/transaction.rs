//! The transaction entity.
//!
//! A transaction is a single movement on an account: a signed amount in cents
//! on a given business date. The sign of `cents` encodes the type
//! (negative = out / expense, positive = in / income). Fields reserved for
//! future features (pairs, recurrences, rules, virtual accounts) are present
//! as `Option` but always `None` in the current step.

use chrono::NaiveDate;

/// A persisted transaction row (the full entity, as stored in `transactions`).
#[derive(Debug, Clone)]
pub struct Transaction {
    /// Opaque identifier (UUID string, application-generated).
    pub id: String,
    /// The user who owns the account this transaction belongs to.
    pub client_id: String,
    /// The account this transaction is recorded on.
    pub account_id: String,
    /// The business date of the movement.
    pub date: NaiveDate,
    /// Signed amount in cents (negative = out, positive = in).
    pub cents: i64,
    /// Short description (e.g. merchant name).
    pub description: Option<String>,
    /// The category this movement is classified under.
    pub category_id: Option<String>,
    /// Free-form note.
    pub note: Option<String>,
    /// (future) transfer pair id.
    pub pair_id: Option<String>,
    /// (future) recurrence rule id.
    pub rec_id: Option<String>,
    /// (future) spending rule id.
    pub rule_id: Option<String>,
    /// (future) whether this row was overridden by a rule.
    pub override_: Option<bool>,
    /// (future) whether this row was auto-computed.
    pub computed: Option<bool>,
    /// (future) virtual account id.
    pub vacct_id: Option<String>,
    /// (future) hash of the import source row.
    pub import_hash: Option<String>,
    /// The user who created this row.
    pub created_by: String,
    /// Creation timestamp, RFC-3339 UTC.
    pub created_at: String,
    /// Last update timestamp, RFC-3339 UTC.
    pub updated_at: String,
}

impl Transaction {
    /// Whether this movement may be moved to a different account.
    ///
    /// Only plain, manually entered movements can. Everything the engines
    /// generate or reconcile keeps its account: transfer legs and card
    /// settlements (`pair_id`, the other leg lives on a specific account),
    /// recurrence and fee-rule output (`rec_id`, `rule_id`, `computed`, which
    /// would be regenerated on the original account), virtual-account
    /// movements (`vacct_id`) and imported rows (`import_hash` is derived from
    /// the account, so a re-import would duplicate the row).
    pub fn can_change_account(&self) -> bool {
        self.pair_id.is_none()
            && self.rec_id.is_none()
            && self.rule_id.is_none()
            && self.vacct_id.is_none()
            && self.import_hash.is_none()
            && self.computed != Some(true)
    }
}

/// A single row as returned by the balance engine: a transaction with its
/// running balance at that point.
#[derive(Debug, Clone)]
pub struct TransactionRow {
    /// The underlying transaction.
    pub tx: Transaction,
    /// The running balance (in cents) after this transaction is applied.
    pub running_balance_cents: i64,
}

/// A day in the register: the date, whether it has any rows, whether it is
/// today, whether it is in the future, the closing balance at end of day,
/// and the list of rows that occurred on that day.
#[derive(Debug, Clone)]
pub struct RegisterDay {
    /// The date of this day.
    pub date: NaiveDate,
    /// Whether this day has at least one movement.
    pub is_empty: bool,
    /// Whether this day is today.
    pub is_today: bool,
    /// Whether this day is in the future.
    pub is_future: bool,
    /// The closing balance at the end of this day (after all rows are applied).
    pub closing_balance_cents: i64,
    /// The movements that occurred on this day (empty if `is_empty`).
    pub rows: Vec<TransactionRow>,
}

/// The summary block returned alongside the register.
#[derive(Debug, Clone, Default)]
pub struct RegisterSummary {
    /// Total income (sum of positive cents) in the period, in cents.
    pub income_cents: i64,
    /// Total expense (absolute sum of negative cents) in the period, in cents.
    pub expense_cents: i64,
    /// Net movement (income - expense) in the period, in cents.
    pub net_cents: i64,
    /// The balance before the first day of the period, in cents.
    pub opening_cents: i64,
    /// The balance after the last day of the period, in cents.
    pub closing_cents: i64,
}

/// The full response from the register endpoint.
#[derive(Debug, Clone)]
pub struct RegisterResult {
    /// The days in the requested period (including empty days).
    pub days: Vec<RegisterDay>,
    /// The summary for this period.
    pub summary: RegisterSummary,
}

/// A logical transfer: a pair of linked transactions on two different accounts.
///
/// The underlying storage is two `Transaction` rows sharing the same `pair_id`.
/// The "from" leg has negative `cents` on the source account; the "to" leg has
/// positive `cents` (same absolute value) on the destination account. This type
/// is the denormalised view returned by the API.
#[derive(Debug, Clone)]
pub struct Transfer {
    /// The shared pair identifier (UUID string).
    pub pair_id: String,
    /// The account funds are moved from.
    pub from_account_id: String,
    /// The account funds are moved to.
    pub to_account_id: String,
    /// The business date of the transfer.
    pub date: NaiveDate,
    /// Absolute amount in cents (always positive).
    pub cents: i64,
    /// Short description.
    pub description: Option<String>,
    /// Free-form note.
    pub note: Option<String>,
    /// The user who created this transfer.
    pub created_by: String,
    /// Creation timestamp, RFC-3339 UTC.
    pub created_at: String,
    /// Last update timestamp, RFC-3339 UTC.
    pub updated_at: String,
}
