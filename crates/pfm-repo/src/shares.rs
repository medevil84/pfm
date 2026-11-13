//! Account share repository: a trait (so the service depends on an abstraction)
//! with a single SQLx-`Any` implementation serving both SQLite and MariaDB.

use async_trait::async_trait;
use pfm_domain::{ShareEntry, ShareLevel};
use sqlx::any::AnyRow;
use sqlx::{AnyPool, AssertSqlSafe, Row};

use crate::Storage;

/// Read/write access to account shares.
#[async_trait]
pub trait SharesRepo: Send + Sync {
    /// Insert (or upsert) a share for `user_id` on `account_id` at the given
    /// `level`. Returns the persisted row.
    async fn upsert(
        &self,
        account_id: &str,
        user_id: &str,
        level: ShareLevel,
    ) -> Result<ShareEntry, sqlx::Error>;

    /// Remove a share by (account_id, user_id). Returns `true` if a row was
    /// removed.
    async fn remove(&self, account_id: &str, user_id: &str) -> Result<bool, sqlx::Error>;

    /// Look up a single share by (account_id, user_id).
    async fn get(&self, account_id: &str, user_id: &str)
        -> Result<Option<ShareEntry>, sqlx::Error>;

    /// All shares on a given account.
    async fn list_for_account(&self, account_id: &str) -> Result<Vec<ShareEntry>, sqlx::Error>;

    /// All accounts shared to a given user.
    async fn list_for_user(&self, user_id: &str) -> Result<Vec<ShareEntry>, sqlx::Error>;

    /// Binary email search: returns `true` only if the given email belongs to a
    /// user who has a share on this account. The response is deliberately
    /// non-revealing: it never exposes names or other user data.
    async fn search_email(&self, account_id: &str, email: &str) -> Result<bool, sqlx::Error>;
}

/// A single SQLx-`Any` implementation of the shares repository, bound to one
/// pool.
#[derive(Clone)]
pub struct SqlxSharesRepo {
    pool: AnyPool,
}

impl SqlxSharesRepo {
    /// Build a repository handle from an existing [`Storage`] (shares its pool).
    pub fn new(storage: &Storage) -> Self {
        Self {
            pool: storage.pool().clone(),
        }
    }
}

impl SqlxSharesRepo {
    /// Ids of the users `user_id` collaborates with: the owners of accounts
    /// shared *with* them and the users they have shared accounts *to*. Their
    /// categories and tags show up on the transactions both sides can see.
    pub async fn related_user_ids(&self, user_id: &str) -> Result<Vec<String>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT a.owner_id AS uid FROM account_shares s \
               JOIN accounts a ON a.id = s.account_id WHERE s.user_id = $1 \
             UNION \
             SELECT s.user_id AS uid FROM account_shares s \
               JOIN accounts a ON a.id = s.account_id WHERE a.owner_id = $1",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(|r| r.try_get::<String, _>("uid")).collect()
    }
}

/// An RFC-3339 UTC timestamp string, as stored in `VARCHAR` columns.
fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

const SHARE_COLUMNS: &str = "id, account_id, user_id, level, created_at";

#[async_trait]
impl SharesRepo for SqlxSharesRepo {
    async fn upsert(
        &self,
        account_id: &str,
        user_id: &str,
        level: ShareLevel,
    ) -> Result<ShareEntry, sqlx::Error> {
        // Check if a row already exists for this (account_id, user_id).
        let existing = self.get(account_id, user_id).await?;

        if let Some(entry) = &existing {
            // Update the level in place.
            sqlx::query(
                "UPDATE account_shares SET level = $1 WHERE account_id = $2 AND user_id = $3",
            )
            .bind(level.as_str())
            .bind(account_id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
            return Ok(ShareEntry {
                level,
                ..entry.clone()
            });
        }

        let id = uuid::Uuid::new_v4().to_string();
        let created_at = now_rfc3339();

        sqlx::query(
            "INSERT INTO account_shares (id, account_id, user_id, level, created_at) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(&id)
        .bind(account_id)
        .bind(user_id)
        .bind(level.as_str())
        .bind(&created_at)
        .execute(&self.pool)
        .await?;

        Ok(ShareEntry {
            id,
            account_id: account_id.to_string(),
            user_id: user_id.to_string(),
            level,
            created_at,
        })
    }

    async fn remove(&self, account_id: &str, user_id: &str) -> Result<bool, sqlx::Error> {
        let result =
            sqlx::query("DELETE FROM account_shares WHERE account_id = $1 AND user_id = $2")
                .bind(account_id)
                .bind(user_id)
                .execute(&self.pool)
                .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn get(
        &self,
        account_id: &str,
        user_id: &str,
    ) -> Result<Option<ShareEntry>, sqlx::Error> {
        let sql = format!(
            "SELECT {SHARE_COLUMNS} FROM account_shares WHERE account_id = $1 AND user_id = $2"
        );
        let row = sqlx::query(AssertSqlSafe(sql))
            .bind(account_id)
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_share).transpose()
    }

    async fn list_for_account(&self, account_id: &str) -> Result<Vec<ShareEntry>, sqlx::Error> {
        let sql = format!("SELECT {SHARE_COLUMNS} FROM account_shares WHERE account_id = $1");
        let rows = sqlx::query(AssertSqlSafe(sql))
            .bind(account_id)
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_share).collect()
    }

    async fn list_for_user(&self, user_id: &str) -> Result<Vec<ShareEntry>, sqlx::Error> {
        let sql = format!("SELECT {SHARE_COLUMNS} FROM account_shares WHERE user_id = $1");
        let rows = sqlx::query(AssertSqlSafe(sql))
            .bind(user_id)
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_share).collect()
    }

    async fn search_email(&self, account_id: &str, email: &str) -> Result<bool, sqlx::Error> {
        let row = sqlx::query(
            "SELECT 1 FROM account_shares s \
             INNER JOIN users u ON u.id = s.user_id \
             WHERE s.account_id = $1 AND u.email = $2 LIMIT 1",
        )
        .bind(account_id)
        .bind(email)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.is_some())
    }
}

/// Map an `account_shares` row into a [`ShareEntry`].
fn row_to_share(row: AnyRow) -> Result<ShareEntry, sqlx::Error> {
    let level: String = row.try_get("level")?;
    Ok(ShareEntry {
        id: row.try_get("id")?,
        account_id: row.try_get("account_id")?,
        user_id: row.try_get("user_id")?,
        level: ShareLevel::parse(&level).unwrap_or(ShareLevel::Read),
        created_at: row.try_get("created_at")?,
    })
}
