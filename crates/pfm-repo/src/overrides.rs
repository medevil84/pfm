//! Recurrence per-date override repository: a trait (so the service depends on
//! an abstraction) with a single SQLx-`Any` implementation serving both SQLite
//! and MariaDB.
//!
//! An override pins a single occurrence date of a rule to a confirmed real
//! amount. The unique key is `(rec_id, date)`; re-confirming a date upserts
//! (the engine-specific `ON CONFLICT` / `ON DUPLICATE KEY` clause is supplied
//! by [`crate::Dialect`]).

use async_trait::async_trait;
use chrono::NaiveDate;
use pfm_domain::RecurrenceOverride;
use sqlx::any::AnyRow;
use sqlx::{AnyPool, AssertSqlSafe, Row};

use crate::dialect::Dialect;
use crate::Storage;

/// Read/write access to per-date recurrence overrides.
#[async_trait]
pub trait OverridesRepo: Send + Sync {
    /// All overrides for a rule, ordered by date.
    async fn list_for_recurrence(
        &self,
        rec_id: &str,
    ) -> Result<Vec<RecurrenceOverride>, sqlx::Error>;

    /// Look up a single override by (rec_id, date).
    async fn get(
        &self,
        rec_id: &str,
        date: NaiveDate,
    ) -> Result<Option<RecurrenceOverride>, sqlx::Error>;

    /// Insert or update the override for (rec_id, date). Returns the persisted
    /// row (the existing row is returned unchanged when it was an update).
    async fn upsert(
        &self,
        rec_id: &str,
        date: NaiveDate,
        amount_cents: i64,
        description: &Option<String>,
    ) -> Result<RecurrenceOverride, sqlx::Error>;

    /// Delete the override for (rec_id, date). Returns `true` if a row was
    /// removed.
    async fn delete(&self, rec_id: &str, date: NaiveDate) -> Result<bool, sqlx::Error>;

    /// Set `amount_cents` on every override of the rule dated `>= from` (all of
    /// them when `from` is `None`). Returns the number of rows updated.
    async fn set_amount_from(
        &self,
        rec_id: &str,
        from: Option<NaiveDate>,
        amount_cents: i64,
    ) -> Result<u64, sqlx::Error>;
}

/// A single SQLx-`Any` implementation of the overrides repository, bound to one
/// pool and engine.
#[derive(Clone)]
pub struct SqlxOverridesRepo {
    pool: AnyPool,
    dialect: Dialect,
}

impl SqlxOverridesRepo {
    /// Build a repository handle from an existing [`Storage`] (shares its pool).
    pub fn new(storage: &Storage) -> Self {
        Self {
            pool: storage.pool().clone(),
            dialect: storage.engine().into(),
        }
    }
}

/// An RFC-3339 UTC timestamp string, as stored in `VARCHAR` columns.
fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Business date as a `YYYY-MM-DD` string.
fn date_iso(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

/// Parse a `YYYY-MM-DD` date string.
fn parse_date(s: &str) -> Result<NaiveDate, sqlx::Error> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|e| sqlx::Error::Decode(Box::new(e)))
}

/// The set of `recurrence_overrides` columns selected by every reading query.
const OVERRIDE_COLUMNS: &str = "id, rec_id, date, amount_cents, description, created_at";

#[async_trait]
impl OverridesRepo for SqlxOverridesRepo {
    async fn list_for_recurrence(
        &self,
        rec_id: &str,
    ) -> Result<Vec<RecurrenceOverride>, sqlx::Error> {
        let sql = format!(
            "SELECT {OVERRIDE_COLUMNS} FROM recurrence_overrides WHERE rec_id = $1 ORDER BY date"
        );
        let rows = sqlx::query(AssertSqlSafe(sql))
            .bind(rec_id)
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_override).collect()
    }

    async fn get(
        &self,
        rec_id: &str,
        date: NaiveDate,
    ) -> Result<Option<RecurrenceOverride>, sqlx::Error> {
        let sql = format!(
            "SELECT {OVERRIDE_COLUMNS} FROM recurrence_overrides WHERE rec_id = $1 AND date = $2"
        );
        let row = sqlx::query(AssertSqlSafe(sql))
            .bind(rec_id)
            .bind(date_iso(date))
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_override).transpose()
    }

    async fn upsert(
        &self,
        rec_id: &str,
        date: NaiveDate,
        amount_cents: i64,
        description: &Option<String>,
    ) -> Result<RecurrenceOverride, sqlx::Error> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = now_rfc3339();

        let base = "INSERT INTO recurrence_overrides \
              (id, rec_id, date, amount_cents, description, created_at) \
              VALUES ($1, $2, $3, $4, $5, $6)";
        let sql = format!(
            "{base} {}",
            self.dialect
                .upsert_suffix("rec_id, date", &["amount_cents", "description"])
        );

        sqlx::query(AssertSqlSafe(sql))
            .bind(&id)
            .bind(rec_id)
            .bind(date_iso(date))
            .bind(amount_cents)
            .bind(description)
            .bind(&now)
            .execute(&self.pool)
            .await?;

        // Re-read so an update path returns the persisted (existing) row.
        self.get(rec_id, date)
            .await?
            .ok_or_else(|| sqlx::Error::RowNotFound)
    }

    async fn delete(&self, rec_id: &str, date: NaiveDate) -> Result<bool, sqlx::Error> {
        let result =
            sqlx::query("DELETE FROM recurrence_overrides WHERE rec_id = $1 AND date = $2")
                .bind(rec_id)
                .bind(date_iso(date))
                .execute(&self.pool)
                .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn set_amount_from(
        &self,
        rec_id: &str,
        from: Option<NaiveDate>,
        amount_cents: i64,
    ) -> Result<u64, sqlx::Error> {
        let result = match from {
            Some(from) => {
                sqlx::query(
                    "UPDATE recurrence_overrides SET amount_cents = $3 \
                     WHERE rec_id = $1 AND date >= $2",
                )
                .bind(rec_id)
                .bind(date_iso(from))
                .bind(amount_cents)
                .execute(&self.pool)
                .await?
            }
            None => {
                sqlx::query("UPDATE recurrence_overrides SET amount_cents = $2 WHERE rec_id = $1")
                    .bind(rec_id)
                    .bind(amount_cents)
                    .execute(&self.pool)
                    .await?
            }
        };
        Ok(result.rows_affected())
    }
}

/// Map a `recurrence_overrides` row into a [`RecurrenceOverride`], decoding the
/// date string.
fn row_to_override(row: AnyRow) -> Result<RecurrenceOverride, sqlx::Error> {
    let date: String = row.try_get("date")?;
    Ok(RecurrenceOverride {
        id: row.try_get("id")?,
        rec_id: row.try_get("rec_id")?,
        date: parse_date(&date)?,
        amount_cents: row.try_get("amount_cents")?,
        description: row.try_get("description")?,
        created_at: row.try_get("created_at")?,
    })
}
