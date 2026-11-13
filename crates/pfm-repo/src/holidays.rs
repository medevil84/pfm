//! Holiday calendar repository: calendars and their ordered rules.

use async_trait::async_trait;
use pfm_domain::RuleParts;
use sqlx::any::AnyRow;
use sqlx::{AnyPool, Row};

use crate::Storage;

/// A calendar's identity (the rules are loaded separately).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarInfo {
    pub code: String,
    pub name: String,
}

#[async_trait]
pub trait HolidaysRepo: Send + Sync {
    async fn list_calendars(&self) -> Result<Vec<CalendarInfo>, sqlx::Error>;
    async fn get_calendar(&self, code: &str) -> Result<Option<CalendarInfo>, sqlx::Error>;
    async fn create_calendar(&self, code: &str, name: &str) -> Result<(), sqlx::Error>;
    async fn rename_calendar(&self, code: &str, name: &str) -> Result<(), sqlx::Error>;
    async fn delete_calendar(&self, code: &str) -> Result<bool, sqlx::Error>;
    /// Users whose preferences select this calendar.
    async fn users_using(&self, code: &str) -> Result<i64, sqlx::Error>;
    /// The calendar's rules in their stored order.
    async fn rules(&self, code: &str) -> Result<Vec<RuleParts>, sqlx::Error>;
    /// Atomically replace the whole rule list.
    async fn replace_rules(&self, code: &str, rules: &[RuleParts]) -> Result<(), sqlx::Error>;
}

#[derive(Clone)]
pub struct SqlxHolidaysRepo {
    pool: AnyPool,
}

impl SqlxHolidaysRepo {
    pub fn new(storage: &Storage) -> Self {
        Self {
            pool: storage.pool().clone(),
        }
    }
}

fn row_to_info(r: &AnyRow) -> Result<CalendarInfo, sqlx::Error> {
    Ok(CalendarInfo {
        code: r.try_get("code")?,
        name: r.try_get("name")?,
    })
}

fn row_to_parts(r: &AnyRow) -> Result<RuleParts, sqlx::Error> {
    Ok(RuleParts {
        label: r.try_get("label")?,
        kind: r.try_get("kind")?,
        month: r.try_get("month")?,
        day: r.try_get("day")?,
        offset_days: r.try_get("offset_days")?,
        weekday: r.try_get("weekday")?,
        nth: r.try_get("nth")?,
        observance: r.try_get("observance")?,
        date: r.try_get("on_date")?,
    })
}

#[async_trait]
impl HolidaysRepo for SqlxHolidaysRepo {
    async fn list_calendars(&self) -> Result<Vec<CalendarInfo>, sqlx::Error> {
        let rows = sqlx::query("SELECT code, name FROM holiday_calendars ORDER BY name")
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(row_to_info).collect()
    }

    async fn get_calendar(&self, code: &str) -> Result<Option<CalendarInfo>, sqlx::Error> {
        let row = sqlx::query("SELECT code, name FROM holiday_calendars WHERE code = $1")
            .bind(code)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(row_to_info).transpose()
    }

    async fn create_calendar(&self, code: &str, name: &str) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT INTO holiday_calendars (code, name, created_at) VALUES ($1, $2, $3)")
            .bind(code)
            .bind(name)
            .bind(chrono::Utc::now().to_rfc3339())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn rename_calendar(&self, code: &str, name: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE holiday_calendars SET name = $2 WHERE code = $1")
            .bind(code)
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn delete_calendar(&self, code: &str) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM holiday_rules WHERE calendar_code = $1")
            .bind(code)
            .execute(&mut *tx)
            .await?;
        let res = sqlx::query("DELETE FROM holiday_calendars WHERE code = $1")
            .bind(code)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(res.rows_affected() > 0)
    }

    async fn users_using(&self, code: &str) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT COUNT(*) FROM user_prefs WHERE holiday_calendar = $1")
            .bind(code)
            .fetch_one(&self.pool)
            .await
    }

    async fn rules(&self, code: &str) -> Result<Vec<RuleParts>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT label, kind, month, day, offset_days, weekday, nth, observance, on_date \
             FROM holiday_rules WHERE calendar_code = $1 ORDER BY sort_order, id",
        )
        .bind(code)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_parts).collect()
    }

    async fn replace_rules(&self, code: &str, rules: &[RuleParts]) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM holiday_rules WHERE calendar_code = $1")
            .bind(code)
            .execute(&mut *tx)
            .await?;
        for (i, r) in rules.iter().enumerate() {
            sqlx::query(
                "INSERT INTO holiday_rules \
                 (id, calendar_code, sort_order, label, kind, month, day, offset_days, weekday, nth, observance, on_date) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
            )
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(code)
            .bind(i as i32)
            .bind(&r.label)
            .bind(&r.kind)
            .bind(r.month)
            .bind(r.day)
            .bind(r.offset_days)
            .bind(r.weekday)
            .bind(r.nth)
            .bind(&r.observance)
            .bind(&r.date)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await
    }
}
