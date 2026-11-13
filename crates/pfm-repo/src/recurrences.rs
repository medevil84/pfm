//! Recurrence-rule repository: a trait (so the service depends on an
//! abstraction) with a single SQLx-`Any` implementation serving both SQLite and
//! MariaDB.
//!
//! Movement / business dates are stored as `YYYY-MM-DD` `VARCHAR`; system
//! timestamps as RFC-3339 `VARCHAR`; booleans as `INTEGER 0/1`. The repository
//! converts at the boundary, keeping one query set for both engines.

use async_trait::async_trait;
use chrono::NaiveDate;
use pfm_domain::{CategoryKind, EndType, Recurrence, RecurrenceMode, WeekendPolicy};
use sqlx::any::AnyRow;
use sqlx::{AnyPool, AssertSqlSafe, Row};

use crate::Storage;

/// Read/write access to recurrence rules.
#[async_trait]
pub trait RecurrencesRepo: Send + Sync {
    /// Insert a new rule. The repository generates the id and the
    /// `created_at` / `updated_at` timestamps. The caller-supplied rule carries
    /// all business fields. Returns the persisted row.
    async fn create(&self, rec: &Recurrence) -> Result<Recurrence, sqlx::Error>;

    /// Look up a rule by id.
    async fn get_by_id(&self, id: &str) -> Result<Option<Recurrence>, sqlx::Error>;

    /// All rules owned by a user, ordered by description.
    async fn list_for_owner(&self, owner_id: &str) -> Result<Vec<Recurrence>, sqlx::Error>;

    /// All rules bound to the given account, ordered by description.
    async fn list_for_account(&self, account_id: &str) -> Result<Vec<Recurrence>, sqlx::Error>;

    /// Replace a rule's mutable fields by id. `kind` is fixed at creation and is
    /// never rewritten.
    async fn update(&self, rec: &Recurrence) -> Result<(), sqlx::Error>;

    /// Delete a rule by id (its overrides are removed by the FK cascade).
    /// Returns `true` if a row was removed.
    async fn delete(&self, id: &str) -> Result<bool, sqlx::Error>;

    /// Every rule, with the date up to which its due occurrences have been
    /// registered (`None` when the scheduler never ran on it).
    async fn list_all_with_watermark(
        &self,
    ) -> Result<Vec<(Recurrence, Option<NaiveDate>)>, sqlx::Error>;

    /// Record that the rule's occurrences are registered up to `date`.
    async fn set_materialized_through(&self, id: &str, date: NaiveDate) -> Result<(), sqlx::Error>;
}

/// A single SQLx-`Any` implementation of the recurrences repository, bound to
/// one pool.
#[derive(Clone)]
pub struct SqlxRecurrencesRepo {
    pool: AnyPool,
}

impl SqlxRecurrencesRepo {
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

/// Business date as a `YYYY-MM-DD` string.
fn date_iso(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

/// Parse a `YYYY-MM-DD` date string.
fn parse_date(s: &str) -> Result<NaiveDate, sqlx::Error> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|e| sqlx::Error::Decode(Box::new(e)))
}

/// The set of `recurrences` columns selected by every rule-reading query.
const RECURRENCE_COLUMNS: &str = "id, owner_id, description, category_id, kind, amount_cents, \
      account_id, pattern, weekend, first_date, end_type, end_date, end_count, mode, avg_n, \
      active, note, linked_account_id, linked_category_id, created_at, updated_at";

#[async_trait]
impl RecurrencesRepo for SqlxRecurrencesRepo {
    async fn create(&self, rec: &Recurrence) -> Result<Recurrence, sqlx::Error> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = now_rfc3339();

        sqlx::query(
            "INSERT INTO recurrences \
              (id, owner_id, description, category_id, kind, amount_cents, account_id, \
               pattern, weekend, first_date, end_type, end_date, end_count, mode, avg_n, \
               active, note, linked_account_id, linked_category_id, created_at, updated_at) \
              VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21)",
        )
        .bind(&id)
        .bind(&rec.owner_id)
        .bind(&rec.description)
        .bind(&rec.category_id)
        .bind(rec.kind.as_str())
        .bind(rec.amount_cents)
        .bind(&rec.account_id)
        .bind(&rec.pattern)
        .bind(rec.weekend.as_str())
        .bind(date_iso(rec.first_date))
        .bind(rec.end_type.as_str())
        .bind(rec.end_date.as_ref().map(|d| date_iso(*d)))
        .bind(rec.end_count)
        .bind(rec.mode.as_str())
        .bind(rec.avg_n)
        .bind(int_bool(rec.active))
        .bind(&rec.note)
        .bind(&rec.linked_account_id)
        .bind(&rec.linked_category_id)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        Ok(Recurrence {
            id,
            created_at: now.clone(),
            updated_at: now,
            ..rec.clone()
        })
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<Recurrence>, sqlx::Error> {
        let sql = format!("SELECT {RECURRENCE_COLUMNS} FROM recurrences WHERE id = $1");
        let row = sqlx::query(AssertSqlSafe(sql))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_recurrence).transpose()
    }

    async fn list_for_owner(&self, owner_id: &str) -> Result<Vec<Recurrence>, sqlx::Error> {
        let sql = format!(
            "SELECT {RECURRENCE_COLUMNS} FROM recurrences WHERE owner_id = $1 ORDER BY description"
        );
        let rows = sqlx::query(AssertSqlSafe(sql))
            .bind(owner_id)
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_recurrence).collect()
    }

    async fn list_for_account(&self, account_id: &str) -> Result<Vec<Recurrence>, sqlx::Error> {
        let sql = format!(
            "SELECT {RECURRENCE_COLUMNS} FROM recurrences WHERE account_id = $1 ORDER BY description"
        );
        let rows = sqlx::query(AssertSqlSafe(sql))
            .bind(account_id)
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_recurrence).collect()
    }

    async fn update(&self, rec: &Recurrence) -> Result<(), sqlx::Error> {
        let updated_at = now_rfc3339();
        sqlx::query(
            "UPDATE recurrences SET \
              description = $2, category_id = $3, amount_cents = $4, account_id = $5, \
              pattern = $6, weekend = $7, first_date = $8, end_type = $9, end_date = $10, \
              end_count = $11, mode = $12, avg_n = $13, active = $14, note = $15, \
              linked_account_id = $16, linked_category_id = $17, updated_at = $18 WHERE id = $1",
        )
        .bind(&rec.id)
        .bind(&rec.description)
        .bind(&rec.category_id)
        .bind(rec.amount_cents)
        .bind(&rec.account_id)
        .bind(&rec.pattern)
        .bind(rec.weekend.as_str())
        .bind(date_iso(rec.first_date))
        .bind(rec.end_type.as_str())
        .bind(rec.end_date.as_ref().map(|d| date_iso(*d)))
        .bind(rec.end_count)
        .bind(rec.mode.as_str())
        .bind(rec.avg_n)
        .bind(int_bool(rec.active))
        .bind(&rec.note)
        .bind(&rec.linked_account_id)
        .bind(&rec.linked_category_id)
        .bind(&updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM recurrences WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn list_all_with_watermark(
        &self,
    ) -> Result<Vec<(Recurrence, Option<NaiveDate>)>, sqlx::Error> {
        let sql = format!(
            "SELECT {RECURRENCE_COLUMNS}, materialized_through FROM recurrences ORDER BY id"
        );
        let rows = sqlx::query(AssertSqlSafe(sql))
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|row| {
                let mark: Option<String> = row.try_get("materialized_through")?;
                let mark = mark.as_deref().map(parse_date).transpose()?;
                Ok((row_to_recurrence(row)?, mark))
            })
            .collect()
    }

    async fn set_materialized_through(&self, id: &str, date: NaiveDate) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE recurrences SET materialized_through = $2 WHERE id = $1")
            .bind(id)
            .bind(date_iso(date))
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

/// Map a `recurrences` row into a [`Recurrence`], decoding the enum strings,
/// the date strings and the `INTEGER` boolean.
fn row_to_recurrence(row: AnyRow) -> Result<Recurrence, sqlx::Error> {
    let kind: String = row.try_get("kind")?;
    let weekend: String = row.try_get("weekend")?;
    let mode: String = row.try_get("mode")?;
    let end_type: String = row.try_get("end_type")?;
    let first_date: String = row.try_get("first_date")?;
    let active: i64 = row.try_get("active")?;

    let end_date: Option<String> = row.try_get("end_date")?;
    let note: String = row.try_get("note")?;

    Ok(Recurrence {
        id: row.try_get("id")?,
        owner_id: row.try_get("owner_id")?,
        description: row.try_get("description")?,
        category_id: row.try_get("category_id")?,
        kind: CategoryKind::parse(&kind).unwrap_or(CategoryKind::Out),
        amount_cents: row.try_get("amount_cents")?,
        account_id: row.try_get("account_id")?,
        pattern: row.try_get("pattern")?,
        weekend: WeekendPolicy::parse(&weekend).unwrap_or(WeekendPolicy::Create),
        first_date: parse_date(&first_date)?,
        end_type: EndType::parse(&end_type).unwrap_or(EndType::Never),
        end_date: match end_date {
            Some(s) => Some(parse_date(&s)?),
            None => None,
        },
        end_count: row.try_get("end_count")?,
        mode: RecurrenceMode::parse(&mode).unwrap_or(RecurrenceMode::Fixed),
        avg_n: row.try_get("avg_n")?,
        active: active != 0,
        note,
        linked_account_id: row.try_get("linked_account_id").unwrap_or(None),
        linked_category_id: row.try_get("linked_category_id").unwrap_or(None),
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}
