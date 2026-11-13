//! Account repository: a trait (so the service depends on an abstraction) with
//! a single SQLx-`Any` implementation serving both SQLite and MariaDB.
//!
//! Queries use positional `$n` placeholders. Business dates are stored as
//! `YYYY-MM-DD` strings and converted to [`chrono::NaiveDate`] at the boundary;
//! system timestamps as RFC-3339 UTC strings; booleans as `INTEGER 0/1`.

use async_trait::async_trait;
use chrono::NaiveDate;
use pfm_domain::{Account, AccountKind};
use sqlx::any::AnyRow;
use sqlx::{AnyPool, AssertSqlSafe, Row};

use crate::Storage;

/// Read/write access to accounts.
#[async_trait]
pub trait AccountsRepo: Send + Sync {
    /// Insert a new account. The repository generates the id, `created_at`, and
    /// `sort_order` (appended at the end of the owner's list). The caller-supplied
    /// `account` carries all business fields; its `id`, `created_at`, and
    /// `sort_order` are overwritten with the generated values and the persisted
    /// row is returned.
    async fn create(&self, owner_id: &str, account: &Account) -> Result<Account, sqlx::Error>;

    /// Look up an account by id.
    async fn get_by_id(&self, id: &str) -> Result<Option<Account>, sqlx::Error>;

    /// All accounts owned by the given user.
    async fn list_by_owner(&self, owner_id: &str) -> Result<Vec<Account>, sqlx::Error>;

    /// All accounts visible to the given user: owned + shared, or all accounts
    /// if `is_admin` is true.
    async fn list_visible(
        &self,
        user_id: &str,
        is_admin: bool,
    ) -> Result<Vec<Account>, sqlx::Error>;

    /// All accounts in the system (admin-only, no permission filtering).
    async fn list_all(&self) -> Result<Vec<Account>, sqlx::Error>;

    /// Replace an account's mutable fields by id.
    async fn update(&self, account: &Account) -> Result<(), sqlx::Error>;

    /// Delete an account by id. Returns `true` if a row was removed.
    async fn delete(&self, id: &str) -> Result<bool, sqlx::Error>;

    /// Whether another account references this account as its backing
    /// (`primary_account_id`). Used to guard deletes.
    async fn referenced_as_primary(&self, id: &str) -> Result<bool, sqlx::Error>;

    /// Reassign all transactions from `from_account_id` to `to_account_id`.
    async fn reassign_transactions(
        &self,
        from_account_id: &str,
        to_account_id: &str,
    ) -> Result<u64, sqlx::Error>;

    /// Whether the given account has any transactions.
    async fn has_transactions(&self, account_id: &str) -> Result<bool, sqlx::Error>;

    /// Update `sort_order` for a batch of accounts. Each element is `(account_id, sort_order)`.
    /// Rows not present in the slice are left untouched.
    async fn reorder_batch(&self, updates: &[(String, i64)]) -> Result<(), sqlx::Error>;

    /// Update the card settlement watermark for a card account.
    /// Sets the watermark to `date` (the sweep recomputes it from scratch, so it
    /// may also move backwards when the settlement semantics change).
    async fn update_last_settled(
        &self,
        account_id: &str,
        date: NaiveDate,
    ) -> Result<(), sqlx::Error>;

    /// All active card accounts in the system (for the periodic full sweep).
    async fn list_active_cards(&self) -> Result<Vec<Account>, sqlx::Error>;
}

/// A single SQLx-`Any` implementation of the accounts repository, bound to one
/// pool.
#[derive(Clone)]
pub struct SqlxAccountsRepo {
    pool: AnyPool,
}

impl SqlxAccountsRepo {
    /// Build a repository handle from an existing [`Storage`] (shares its pool).
    pub fn new(storage: &Storage) -> Self {
        Self {
            pool: storage.pool().clone(),
        }
    }
}

/// An RFC-3339 UTC timestamp string, as stored in `VARCHAR` columns.
fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Encode a `bool` as the `INTEGER 0/1` value the `Any` driver round-trips.
fn int_bool(b: bool) -> i64 {
    if b {
        1
    } else {
        0
    }
}

/// Format a business date for storage as `YYYY-MM-DD`.
fn date_iso(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

/// Parse a `YYYY-MM-DD` string into a [`NaiveDate`].
fn parse_date(s: &str) -> Result<NaiveDate, sqlx::Error> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|e| sqlx::Error::Decode(Box::new(e)))
}

/// The set of `accounts` columns selected by every account-reading query.
const ACCOUNT_COLUMNS: &str = "id, owner_id, name, kind, icon, note, iban, currency, \
     start_date, start_balance_cents, limit_cents, charge_day, closing_day, primary_account_id, \
     rate_pct_bps, goal_cents, min_balance_cents, active, created_at, sort_order, \
     last_settled_date";

#[async_trait]
impl AccountsRepo for SqlxAccountsRepo {
    async fn create(&self, owner_id: &str, account: &Account) -> Result<Account, sqlx::Error> {
        let id = uuid::Uuid::new_v4().to_string();
        let created_at = now_rfc3339();

        // Place the new account at the end of the owner's list.
        let max_row = sqlx::query(
            "SELECT COALESCE(MAX(sort_order), -1) AS m FROM accounts WHERE owner_id = $1",
        )
        .bind(owner_id)
        .fetch_one(&self.pool)
        .await?;
        let sort_order: i64 = max_row.try_get("m").unwrap_or(-1) + 1;

        sqlx::query(
            "INSERT INTO accounts \
             (id, owner_id, name, kind, icon, note, iban, currency, start_date, \
              start_balance_cents, limit_cents, charge_day, closing_day, primary_account_id, \
              rate_pct_bps, goal_cents, min_balance_cents, active, created_at, sort_order) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20)",
        )
        .bind(&id)
        .bind(owner_id)
        .bind(&account.name)
        .bind(account.kind.as_str())
        .bind(&account.icon)
        .bind(&account.note)
        .bind(&account.iban)
        .bind(&account.currency)
        .bind(date_iso(account.start_date))
        .bind(account.start_balance_cents)
        .bind(account.limit_cents)
        .bind(account.charge_day)
        .bind(account.closing_day)
        .bind(&account.primary_account_id)
        .bind(account.rate_pct_bps)
        .bind(account.goal_cents)
        .bind(account.min_balance_cents)
        .bind(int_bool(account.active))
        .bind(&created_at)
        .bind(sort_order)
        .execute(&self.pool)
        .await?;

        Ok(Account {
            id,
            owner_id: owner_id.to_string(),
            created_at,
            sort_order,
            ..account.clone()
        })
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<Account>, sqlx::Error> {
        let sql = format!("SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE id = $1");
        let row = sqlx::query(AssertSqlSafe(sql))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_account).transpose()
    }

    async fn list_by_owner(&self, owner_id: &str) -> Result<Vec<Account>, sqlx::Error> {
        let sql = format!(
            "SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE owner_id = $1 ORDER BY sort_order ASC, name ASC"
        );
        let rows = sqlx::query(AssertSqlSafe(sql))
            .bind(owner_id)
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_account).collect()
    }

    async fn list_visible(
        &self,
        user_id: &str,
        is_admin: bool,
    ) -> Result<Vec<Account>, sqlx::Error> {
        let admin_flag = if is_admin { "1" } else { "0" };
        let sql = format!(
            "SELECT {ACCOUNT_COLUMNS} FROM accounts \
             WHERE owner_id = $1 \
                OR id IN (SELECT account_id FROM account_shares WHERE user_id = $1) \
                OR $2 = '1' \
             ORDER BY sort_order ASC, name ASC"
        );
        let rows = sqlx::query(AssertSqlSafe(sql))
            .bind(user_id)
            .bind(admin_flag)
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_account).collect()
    }

    async fn update(&self, account: &Account) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE accounts SET \
             name = $2, kind = $3, icon = $4, note = $5, iban = $6, currency = $7, \
             start_date = $8, start_balance_cents = $9, limit_cents = $10, charge_day = $11, \
             closing_day = $12, primary_account_id = $13, rate_pct_bps = $14, goal_cents = $15, \
             min_balance_cents = $16, active = $17, sort_order = $18 \
             WHERE id = $1",
        )
        .bind(&account.id)
        .bind(&account.name)
        .bind(account.kind.as_str())
        .bind(&account.icon)
        .bind(&account.note)
        .bind(&account.iban)
        .bind(&account.currency)
        .bind(date_iso(account.start_date))
        .bind(account.start_balance_cents)
        .bind(account.limit_cents)
        .bind(account.charge_day)
        .bind(account.closing_day)
        .bind(&account.primary_account_id)
        .bind(account.rate_pct_bps)
        .bind(account.goal_cents)
        .bind(account.min_balance_cents)
        .bind(int_bool(account.active))
        .bind(account.sort_order)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM accounts WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn referenced_as_primary(&self, id: &str) -> Result<bool, sqlx::Error> {
        let row = sqlx::query(
            "SELECT 1 FROM accounts WHERE primary_account_id = $1 AND id <> $2 LIMIT 1",
        )
        .bind(id)
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.is_some())
    }

    async fn list_all(&self) -> Result<Vec<Account>, sqlx::Error> {
        let sql =
            format!("SELECT {ACCOUNT_COLUMNS} FROM accounts ORDER BY sort_order ASC, name ASC");
        let rows = sqlx::query(AssertSqlSafe(sql))
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_account).collect()
    }

    async fn reassign_transactions(
        &self,
        from_account_id: &str,
        to_account_id: &str,
    ) -> Result<u64, sqlx::Error> {
        let result = sqlx::query("UPDATE transactions SET account_id = $1 WHERE account_id = $2")
            .bind(to_account_id)
            .bind(from_account_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    async fn has_transactions(&self, account_id: &str) -> Result<bool, sqlx::Error> {
        let row = sqlx::query("SELECT 1 FROM transactions WHERE account_id = $1 LIMIT 1")
            .bind(account_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some())
    }

    async fn reorder_batch(&self, updates: &[(String, i64)]) -> Result<(), sqlx::Error> {
        for (id, order) in updates {
            sqlx::query("UPDATE accounts SET sort_order = $1 WHERE id = $2")
                .bind(order)
                .bind(id)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    async fn update_last_settled(
        &self,
        account_id: &str,
        date: NaiveDate,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE accounts \
             SET last_settled_date = $2 \
             WHERE id = $1",
        )
        .bind(account_id)
        .bind(date_iso(date))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_active_cards(&self) -> Result<Vec<Account>, sqlx::Error> {
        let sql = format!(
            "SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE kind = 'card' AND active = 1 ORDER BY sort_order ASC, name ASC"
        );
        let rows = sqlx::query(AssertSqlSafe(sql))
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_account).collect()
    }
}

/// Map an `accounts` row into an [`Account`], decoding the `INTEGER` boolean,
/// the kind string, and the date / timestamp strings.
fn row_to_account(row: AnyRow) -> Result<Account, sqlx::Error> {
    let kind: String = row.try_get("kind")?;
    let active: i64 = row.try_get("active")?;
    let start_date: String = row.try_get("start_date")?;
    let last_settled: String = row.try_get("last_settled_date")?;
    Ok(Account {
        id: row.try_get("id")?,
        owner_id: row.try_get("owner_id")?,
        name: row.try_get("name")?,
        kind: AccountKind::parse(&kind).unwrap_or(AccountKind::Current),
        icon: row.try_get("icon")?,
        note: row.try_get("note")?,
        iban: row.try_get("iban")?,
        currency: row
            .try_get::<String, _>("currency")
            .unwrap_or_else(|_| "EUR".to_string()),
        start_date: parse_date(&start_date)?,
        start_balance_cents: row.try_get("start_balance_cents")?,
        limit_cents: row.try_get("limit_cents")?,
        charge_day: row.try_get("charge_day")?,
        closing_day: row.try_get("closing_day")?,
        primary_account_id: row.try_get("primary_account_id")?,
        rate_pct_bps: row.try_get("rate_pct_bps")?,
        goal_cents: row.try_get("goal_cents")?,
        min_balance_cents: row.try_get("min_balance_cents")?,
        active: active != 0,
        created_at: row.try_get("created_at")?,
        sort_order: row.try_get("sort_order").unwrap_or(0),
        last_settled_date: parse_date(&last_settled)?,
    })
}
