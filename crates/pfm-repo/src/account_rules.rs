//! Spending-rule repository: a trait (so the service depends on an
//! abstraction) with a single SQLx-`Any` implementation serving both SQLite and
//! MariaDB.
//!
//! Business dates are `YYYY-MM-DD` `VARCHAR`; system timestamps as RFC-3339
//! `VARCHAR`; booleans as `INTEGER 0/1`; the string enums round-trip through
//! the pure domain types. One query set serves both engines.

use async_trait::async_trait;
use pfm_domain::{AccountRule, Cadence, CalcType, CondBase, CondOp};
use sqlx::any::AnyRow;
use sqlx::{AnyPool, AssertSqlSafe, Row};

use crate::Storage;

/// Read/write access to spending rules.
#[async_trait]
pub trait AccountRulesRepo: Send + Sync {
    /// Insert a new rule. The repository generates the id and the
    /// `created_at` / `updated_at` timestamps. Returns the persisted row.
    async fn create(&self, rule: &AccountRule) -> Result<AccountRule, sqlx::Error>;

    /// Look up a rule by id.
    async fn get_by_id(&self, id: &str) -> Result<Option<AccountRule>, sqlx::Error>;

    /// All rules bound to the given account, ordered by name.
    async fn list_for_account(&self, account_id: &str) -> Result<Vec<AccountRule>, sqlx::Error>;

    /// Replace a rule's mutable fields by id.
    async fn update(&self, rule: &AccountRule) -> Result<(), sqlx::Error>;

    /// Delete a rule by id. Returns `true` if a row was removed.
    async fn delete(&self, id: &str) -> Result<bool, sqlx::Error>;
}

/// A single SQLx-`Any` implementation of the rules repository, bound to one
/// pool.
#[derive(Clone)]
pub struct SqlxAccountRulesRepo {
    pool: AnyPool,
}

impl SqlxAccountRulesRepo {
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

/// The set of `account_rules` columns selected by every rule-reading query.
const RULE_COLUMNS: &str = "id, account_id, name, category_id, calc_type, value, is_annual, \
      prorata_days, annual_cap_cents, cadence, charge_day, charge_month, cond_base, cond_op, cond_threshold, \
      active, created_at, updated_at";

#[async_trait]
impl AccountRulesRepo for SqlxAccountRulesRepo {
    async fn create(&self, rule: &AccountRule) -> Result<AccountRule, sqlx::Error> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = now_rfc3339();

        sqlx::query(
            "INSERT INTO account_rules \
              (id, account_id, name, category_id, calc_type, value, is_annual, \
               annual_cap_cents, cadence, charge_day, charge_month, cond_base, cond_op, \
               cond_threshold, active, created_at, updated_at, prorata_days) \
              VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)",
        )
        .bind(&id)
        .bind(&rule.account_id)
        .bind(&rule.name)
        .bind(&rule.category_id)
        .bind(rule.calc_type.as_str())
        .bind(rule.value)
        .bind(int_bool(rule.is_annual))
        .bind(rule.annual_cap_cents)
        .bind(rule.cadence.as_str())
        .bind(rule.charge_day as i64)
        .bind(rule.charge_month.map(|m| m as i64))
        .bind(rule.cond_base.as_str())
        .bind(rule.cond_op.map(|o| o.as_str().to_string()))
        .bind(rule.cond_threshold)
        .bind(int_bool(rule.active))
        .bind(&now)
        .bind(&now)
        .bind(int_bool(rule.prorata_days))
        .execute(&self.pool)
        .await?;

        Ok(AccountRule {
            id,
            created_at: now.clone(),
            updated_at: now,
            ..rule.clone()
        })
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<AccountRule>, sqlx::Error> {
        let sql = format!("SELECT {RULE_COLUMNS} FROM account_rules WHERE id = $1");
        let row = sqlx::query(AssertSqlSafe(sql))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_rule).transpose()
    }

    async fn list_for_account(&self, account_id: &str) -> Result<Vec<AccountRule>, sqlx::Error> {
        let sql =
            format!("SELECT {RULE_COLUMNS} FROM account_rules WHERE account_id = $1 ORDER BY name");
        let rows = sqlx::query(AssertSqlSafe(sql))
            .bind(account_id)
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_rule).collect()
    }

    async fn update(&self, rule: &AccountRule) -> Result<(), sqlx::Error> {
        let updated_at = now_rfc3339();
        sqlx::query(
            "UPDATE account_rules SET \
              account_id = $2, name = $3, category_id = $4, calc_type = $5, value = $6, \
              is_annual = $7, annual_cap_cents = $8, cadence = $9, charge_day = $10, \
              charge_month = $11, cond_base = $12, cond_op = $13, cond_threshold = $14, \
              active = $15, updated_at = $16, prorata_days = $17 WHERE id = $1",
        )
        .bind(&rule.id)
        .bind(&rule.account_id)
        .bind(&rule.name)
        .bind(&rule.category_id)
        .bind(rule.calc_type.as_str())
        .bind(rule.value)
        .bind(int_bool(rule.is_annual))
        .bind(rule.annual_cap_cents)
        .bind(rule.cadence.as_str())
        .bind(rule.charge_day as i64)
        .bind(rule.charge_month.map(|m| m as i64))
        .bind(rule.cond_base.as_str())
        .bind(rule.cond_op.map(|o| o.as_str().to_string()))
        .bind(rule.cond_threshold)
        .bind(int_bool(rule.active))
        .bind(&updated_at)
        .bind(int_bool(rule.prorata_days))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM account_rules WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

/// Map an `account_rules` row into an [`AccountRule`], decoding the enum
/// strings and the `INTEGER` booleans.
fn row_to_rule(row: AnyRow) -> Result<AccountRule, sqlx::Error> {
    let calc_type: String = row.try_get("calc_type")?;
    let cadence: String = row.try_get("cadence")?;
    let cond_base: String = row.try_get("cond_base")?;
    let is_annual: i64 = row.try_get("is_annual")?;
    let prorata_days: i64 = row.try_get("prorata_days")?;
    let active: i64 = row.try_get("active")?;
    let charge_month: Option<i64> = row.try_get("charge_month")?;
    let cond_op: Option<String> = row.try_get("cond_op")?;

    Ok(AccountRule {
        id: row.try_get("id")?,
        account_id: row.try_get("account_id")?,
        name: row.try_get("name")?,
        category_id: row.try_get("category_id")?,
        calc_type: CalcType::parse(&calc_type).unwrap_or(CalcType::Fixed),
        value: row.try_get("value")?,
        is_annual: is_annual != 0,
        prorata_days: prorata_days != 0,
        annual_cap_cents: row.try_get("annual_cap_cents")?,
        cadence: Cadence::parse(&cadence).unwrap_or(Cadence::Monthly),
        charge_day: row.try_get::<i64, _>("charge_day")? as u32,
        charge_month: charge_month.map(|m| m as u32),
        cond_base: CondBase::parse(&cond_base).unwrap_or(CondBase::Always),
        cond_op: cond_op.as_deref().and_then(CondOp::parse),
        cond_threshold: row.try_get("cond_threshold")?,
        active: active != 0,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}
