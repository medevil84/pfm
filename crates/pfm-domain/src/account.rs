//! The account entity and its kind.
//!
//! An account is one of four kinds, each with its own set of optional,
//! kind-specific fields. The kind determines which fields are meaningful and
//! which are required; the service layer enforces those rules. Amounts are
//! carried as `i64` cents; `start_date` is a business date (`NaiveDate`) while
//! `created_at` is a system timestamp kept as an RFC-3339 UTC string so the
//! type stays `serde`-free.

use chrono::NaiveDate;

/// The kind of account. Drives the set of required / meaningful fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccountKind {
    /// A standard checking / current account.
    Current,
    /// An interest-bearing deposit account (carries a rate and a backing
    /// current account).
    Deposit,
    /// A credit card (carries a spending limit and a charge day).
    Card,
    /// A virtual "piggy bank" account (may carry a goal and a backing account).
    Virtual,
}

impl AccountKind {
    /// Stable string form used on the wire and in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            AccountKind::Current => "current",
            AccountKind::Deposit => "deposit",
            AccountKind::Card => "card",
            AccountKind::Virtual => "virtual",
        }
    }

    /// Parse a kind from its string form. `None` for unknown values.
    pub fn parse(s: &str) -> Option<AccountKind> {
        Some(match s {
            "current" => AccountKind::Current,
            "deposit" => AccountKind::Deposit,
            "card" => AccountKind::Card,
            "virtual" => AccountKind::Virtual,
            _ => return None,
        })
    }
}

impl std::fmt::Display for AccountKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A persisted account row (the full entity, as stored in `accounts`).
///
/// The kind-specific optional fields are all present on the struct; the
/// service layer decides which apply to a given kind. Fields that are
/// irrelevant to the account's kind are `None`.
#[derive(Debug, Clone)]
pub struct Account {
    /// Opaque identifier (UUID string, application-generated).
    pub id: String,
    /// The user who owns this account.
    pub owner_id: String,
    /// Display name.
    pub name: String,
    pub kind: AccountKind,
    /// Bootstrap icon name (e.g. `"bi-bank"`).
    pub icon: String,
    /// Free-form note shown under the name in the sidebar.
    pub note: Option<String>,
    /// IBAN (current / deposit accounts).
    pub iban: Option<String>,
    /// ISO 4217 currency code.
    pub currency: String,
    /// The date the account was opened.
    pub start_date: NaiveDate,
    /// Opening balance in cents.
    pub start_balance_cents: i64,
    /// (card) spending limit in cents.
    pub limit_cents: Option<i64>,
    /// (card) day of month the statement is charged, 1..=28.
    pub charge_day: Option<i32>,
    /// (card) day of month the accounting cycle closes, 1..=28. The charge
    /// falls on `charge_day` of the same month when `charge_day >= closing_day`,
    /// otherwise on `charge_day` of the following month.
    pub closing_day: Option<i32>,
    /// (deposit / virtual) the backing current account.
    pub primary_account_id: Option<String>,
    /// (deposit) gross annual rate in basis points (e.g. 300 = 3.00%).
    pub rate_pct_bps: Option<i64>,
    /// (virtual) the savings goal in cents.
    pub goal_cents: Option<i64>,
    /// (deposit) minimum balance in cents.
    pub min_balance_cents: Option<i64>,
    /// Whether the account is active (vs. archived).
    pub active: bool,
    /// Creation timestamp, RFC-3339 UTC.
    pub created_at: String,
    /// User-defined display order (lower = first). Secondary sort is by name.
    pub sort_order: i64,
    /// (card) the most recent charge date that has been settled.
    /// Acts as a watermark for incremental catch-up. Default `1900-01-01`.
    pub last_settled_date: NaiveDate,
}
