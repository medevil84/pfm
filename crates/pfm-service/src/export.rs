//! CSV export use-cases: build the fixed-format register CSV for an account
//! over a date range, applying the same permission rule (Step 4: `can_see`)
//! and the same filters as the register (Step 5: `GET /transactions`).
//!
//! The service is responsible for the *data*: it enforces read access, fetches
//! the filtered rows, computes the running balance (opening balance + prefix
//! sum, exactly as the register does), resolves category names, and hands a
//! vector of [`ExportRow`]s to the pure [`format_export_csv`] formatter. The
//! pure byte/line formatting (separators, date form, decimal rendering, BOM)
//! lives in `pfm-domain` so it can be tested without a database.

use std::collections::BTreeMap;

use chrono::NaiveDate;
use pfm_domain::{can_see, format_export_csv, Account, ExportRow, Role};
use pfm_repo::{
    AccountsRepo, CategoriesRepo, RegisterFilters, SharesRepo, SqlxAccountsRepo, SqlxAuthRepo,
    SqlxCategoriesRepo, SqlxSharesRepo, SqlxTransactionsRepo, Storage, TransactionsRepo, UsersRepo,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExportError {
    #[error("account not found")]
    AccountNotFound,
    #[error("forbidden")]
    Forbidden,
    #[error("validation error: {0}")]
    Validation(String),
    #[error("internal error: {0}")]
    Internal(String),
}

/// The export payload returned to the transport layer: the CSV body and the
/// suggested download filename (used for the `Content-Disposition` header).
#[derive(Debug, Clone)]
pub struct ExportOutput {
    /// Suggested file name for the download (sanitized, `.csv`).
    pub filename: String,
    /// The full CSV document (BOM-prefixed, `;`-separated, `\r\n` lines).
    pub csv: String,
}

pub struct ExportService {
    transactions: SqlxTransactionsRepo,
    accounts: SqlxAccountsRepo,
    users: SqlxAuthRepo,
    shares: SqlxSharesRepo,
    categories: SqlxCategoriesRepo,
}

impl ExportService {
    pub fn new(storage: &Storage) -> Self {
        Self {
            transactions: SqlxTransactionsRepo::new(storage),
            accounts: SqlxAccountsRepo::new(storage),
            users: SqlxAuthRepo::new(storage),
            shares: SqlxSharesRepo::new(storage),
            categories: SqlxCategoriesRepo::new(storage),
        }
    }

    /// Read-access check mirroring the register (Step 4): the actor may read an
    /// account when they are the owner, an admin, or hold any share entry.
    async fn can_see_account(
        &self,
        actor_id: &str,
        account: &Account,
    ) -> Result<bool, ExportError> {
        if account.owner_id == actor_id {
            return Ok(true);
        }

        let actor = self
            .users
            .get_by_id(actor_id)
            .await
            .map_err(|e| ExportError::Internal(e.to_string()))?
            .ok_or_else(|| ExportError::Internal("actor not found".into()))?;

        if actor.role == Role::Admin {
            return Ok(true);
        }

        let has_share = self
            .shares
            .get(&account.id, actor_id)
            .await
            .map_err(|e| ExportError::Internal(e.to_string()))?
            .is_some();

        Ok(can_see(actor.role, actor_id, &account.owner_id, has_share))
    }

    /// Build the fixed-format CSV export for `account_id` over `[from, to]`,
    /// applying `filters` (category / type / min-max / text / piggy selection)
    /// exactly as the register does. Read access to the account is required.
    ///
    /// The effective range is clamped to the account's `start_date` (as in the
    /// register), and the per-row running balance starts from the account's
    /// opening balance plus the prefix sum of the (piggy-filtered) rows that
    /// precede the range, so the exported balance column is consistent with the
    /// register the user is looking at.
    pub async fn export(
        &self,
        actor_id: &str,
        account_id: &str,
        from: NaiveDate,
        to: NaiveDate,
        filters: &RegisterFilters,
    ) -> Result<ExportOutput, ExportError> {
        if from > to {
            return Err(ExportError::Validation("from must be <= to".into()));
        }

        let account = self
            .accounts
            .get_by_id(account_id)
            .await
            .map_err(|e| ExportError::Internal(e.to_string()))?
            .ok_or(ExportError::AccountNotFound)?;

        if !self.can_see_account(actor_id, &account).await? {
            return Err(ExportError::Forbidden);
        }

        let effective_from = from.max(account.start_date);
        let effective_to = to.max(effective_from);

        let rows = self
            .transactions
            .list_for_register(account_id, effective_from, effective_to, filters)
            .await
            .map_err(|e| ExportError::Internal(e.to_string()))?;

        // Opening balance consistent with the register's visible rows.
        let pre_sum = self
            .transactions
            .sum_cents_before_filtered(
                account_id,
                effective_from,
                filters.vacct_included.as_deref(),
            )
            .await
            .map_err(|e| ExportError::Internal(e.to_string()))?;
        let mut running = account.start_balance_cents + pre_sum;

        // Category id → name map for the (human-readable) category column.
        let all_cats = self
            .categories
            .list()
            .await
            .map_err(|e| ExportError::Internal(e.to_string()))?;
        let cat_name: BTreeMap<&str, &str> = all_cats
            .iter()
            .map(|c| (c.id.as_str(), c.name.as_str()))
            .collect();

        let export_rows: Vec<ExportRow> = rows
            .iter()
            .map(|tx| {
                running += tx.cents;
                let category = tx
                    .category_id
                    .as_deref()
                    .and_then(|id| cat_name.get(id).copied())
                    .unwrap_or_default();
                ExportRow {
                    date: tx.date,
                    description: tx.description.clone().unwrap_or_default(),
                    category: category.to_string(),
                    cents: tx.cents,
                    running_balance_cents: running,
                }
            })
            .collect();

        let filename = export_filename(&account, to);
        Ok(ExportOutput {
            filename,
            csv: format_export_csv(&export_rows),
        })
    }
}

/// Build a filesystem-safe download name from the account's display name
/// (falling back to its id) plus the export end date.
fn export_filename(account: &Account, to: NaiveDate) -> String {
    let sanitized: String = account
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();

    let base = if sanitized.is_empty() {
        account.id.clone()
    } else {
        sanitized
    };
    format!("{base}_{}.csv", to.format("%Y-%m-%d"))
}
