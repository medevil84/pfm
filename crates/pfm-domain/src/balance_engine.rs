//! Pure balance-engine: transforms a flat list of transaction rows (with
//! running balances) into a per-day register with empty days filled.
//!
//! This module is intentionally free of I/O, serialization, or framework
//! dependencies. The caller (the service layer) queries the database for the
//! rows (with the SQL window function computing `running_balance_cents`) and
//! passes them here along with the opening balance and date range.

use chrono::NaiveDate;

use crate::transaction::{
    RegisterDay, RegisterResult, RegisterSummary, Transaction, TransactionRow,
};

/// Build the per-day register from a set of transaction rows.
///
/// # Arguments
/// * `opening_balance` - the balance before the first day in the range.
/// * `rows` - transaction rows, sorted by (date, id). Each carries its
///   `running_balance_cents` as computed by the SQL window function.
/// * `from` - the first date to include in the register.
/// * `to` - the last date to include in the register.
/// * `today` - today's date (for the `is_today` / `is_future` flags).
/// * `show_future` - if `false`, days after `today` are truncated.
pub fn build_register(
    opening_balance: i64,
    rows: Vec<TransactionRow>,
    from: NaiveDate,
    to: NaiveDate,
    today: NaiveDate,
    show_future: bool,
) -> RegisterResult {
    let effective_to = if show_future { to } else { to.min(today) };

    let from = from.min(effective_to);
    let day_count = (effective_to - from).num_days() as usize + 1;

    // Group rows by date.
    let mut rows_by_date: std::collections::BTreeMap<NaiveDate, Vec<TransactionRow>> =
        std::collections::BTreeMap::new();
    for row in rows {
        if row.tx.date < from || row.tx.date > effective_to {
            continue;
        }
        rows_by_date.entry(row.tx.date).or_default().push(row);
    }

    // Track income / expense for the summary.
    let mut income_cents: i64 = 0;
    let mut expense_cents: i64 = 0;
    for date_rows in rows_by_date.values() {
        for row in date_rows {
            if row.tx.cents > 0 {
                income_cents += row.tx.cents;
            } else {
                expense_cents += -row.tx.cents;
            }
        }
    }

    // Walk each day in the range.
    let mut days: Vec<RegisterDay> = Vec::with_capacity(day_count);
    let mut current_balance = opening_balance;

    for offset in 0..=day_count.saturating_sub(1) {
        let date = from + chrono::Duration::days(offset as i64);

        if let Some(day_rows) = rows_by_date.get(&date) {
            // The closing balance is the running balance of the last row.
            let closing = day_rows
                .last()
                .map(|r| r.running_balance_cents)
                .unwrap_or(current_balance);
            current_balance = closing;

            days.push(RegisterDay {
                date,
                is_empty: false,
                is_today: date == today,
                is_future: date > today,
                closing_balance_cents: closing,
                rows: std::mem::take(rows_by_date.get_mut(&date).unwrap()),
            });
        } else {
            days.push(RegisterDay {
                date,
                is_empty: true,
                is_today: date == today,
                is_future: date > today,
                closing_balance_cents: current_balance,
                rows: Vec::new(),
            });
        }
    }

    let closing_cents = days
        .last()
        .map(|d| d.closing_balance_cents)
        .unwrap_or(opening_balance);
    let net_cents = income_cents - expense_cents;

    let summary = RegisterSummary {
        income_cents,
        expense_cents,
        net_cents,
        opening_cents: opening_balance,
        closing_cents,
    };

    RegisterResult { days, summary }
}

/// The two balances of an account that feeds one or more piggy-bank (virtual)
/// accounts. A piggy bank is a *slice* of a real account: moving money into it
/// is a pair of linked movements that set `vacct_id` on both legs. The real
/// account therefore carries two distinct balances.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualBalances {
    /// The balance the bank actually shows: the opening balance plus the
    /// movements that are not part of any piggy allocation
    /// (`vacct_id IS NULL`). Every set-aside toward a piggy bank is ignored.
    pub real_cents: i64,
    /// The "available" balance: the opening balance plus the non-allocated
    /// movements and the allocation legs belonging to the piggy banks in
    /// `selected`. With no piggy selected this collapses to [`real_cents`].
    pub allocated_cents: i64,
}

/// Derive the two balances from the account's flat list of movements.
///
/// `rows` is every movement on the account (the opening balance is excluded
/// and supplied separately as `start_balance_cents`). A movement participates
/// in the *real* balance only when it is not an allocation (`vacct_id` is
/// `None`). A movement participates in the *allocated* balance when it is not
/// an allocation, or when its piggy bank (`vacct_id`) is one of `selected`.
///
/// This is the pure, DB-free counterpart of the register's per-balance view:
/// deselecting a piggy bank simply drops it from `selected`, which removes its
/// allocation legs from the allocated balance exactly as if the piggy bank had
/// never existed.
pub fn virtual_balances(
    start_balance_cents: i64,
    rows: &[Transaction],
    selected: &[String],
) -> VirtualBalances {
    let mut real_cents = start_balance_cents;
    let mut allocated_cents = start_balance_cents;

    for row in rows {
        // A row is an allocation when it carries a piggy-bank id; it belongs
        // to a *selected* piggy when that id is in `selected`.
        let selected_allocation = row
            .vacct_id
            .as_deref()
            .is_some_and(|v| selected.iter().any(|s| s == v));

        if row.vacct_id.is_none() {
            // Not an allocation: counts in both balances.
            real_cents += row.cents;
            allocated_cents += row.cents;
        } else if selected_allocation {
            // An allocation of a selected piggy bank: folds back into the
            // available balance only.
            allocated_cents += row.cents;
        }
        // Allocations of a piggy that is not selected contribute to neither:
        // the bank balance ignores them and the available balance hides them.
    }

    VirtualBalances {
        real_cents,
        allocated_cents,
    }
}
