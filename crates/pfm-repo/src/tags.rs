//! Tag repository: tag CRUD plus the many-to-many assignment to transactions.

use async_trait::async_trait;
use sqlx::any::AnyRow;
use sqlx::{AnyPool, AssertSqlSafe, Row};

use crate::Storage;

/// A tag: a short unique label with a free-text description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub id: String,
    pub label: String,
    pub description: String,
    /// Optional CSS colour (`#rrggbb`); empty = default.
    pub color: String,
    pub created_at: String,
    /// The user who created it; `None` = shared (rows that predate per-user
    /// ownership), changeable by an administrator only.
    pub owner_id: Option<String>,
}

#[async_trait]
pub trait TagsRepo: Send + Sync {
    async fn create(&self, tag: &Tag) -> Result<Tag, sqlx::Error>;
    async fn get_by_id(&self, id: &str) -> Result<Option<Tag>, sqlx::Error>;
    /// A tag with this label (case-insensitive) among those of `owner_id`
    /// (`None` = the shared ones). Labels are unique per owner, not globally.
    async fn find_by_label(
        &self,
        owner_id: Option<&str>,
        label: &str,
    ) -> Result<Option<Tag>, sqlx::Error>;
    /// Shared tags plus those owned by any of `owner_ids`, ordered by label.
    /// `None` = every tag (administrators).
    async fn list_visible(&self, owner_ids: Option<&[String]>) -> Result<Vec<Tag>, sqlx::Error>;
    /// Make every tag owned by `owner_id` shared (that user is being deleted).
    async fn disown(&self, owner_id: &str) -> Result<u64, sqlx::Error>;
    async fn update(&self, tag: &Tag) -> Result<(), sqlx::Error>;
    async fn delete(&self, id: &str) -> Result<bool, sqlx::Error>;
    /// `(transaction_id, tag_id)` pairs for the transactions of the given accounts.
    async fn assignments_for_accounts(
        &self,
        account_ids: &[String],
    ) -> Result<Vec<(String, String)>, sqlx::Error>;
    /// Replace the whole tag set of a transaction.
    async fn set_for_transaction(
        &self,
        transaction_id: &str,
        tag_ids: &[String],
    ) -> Result<(), sqlx::Error>;
    /// Add and remove tags on several transactions in one database transaction.
    async fn bulk_change(
        &self,
        transaction_ids: &[String],
        add: &[String],
        remove: &[String],
    ) -> Result<(), sqlx::Error>;
}

#[derive(Clone)]
pub struct SqlxTagsRepo {
    pool: AnyPool,
}

impl SqlxTagsRepo {
    pub fn new(storage: &Storage) -> Self {
        Self {
            pool: storage.pool().clone(),
        }
    }
}

const TAG_COLUMNS: &str = "id, label, description, color, created_at, owner_id";

fn row_to_tag(row: &AnyRow) -> Result<Tag, sqlx::Error> {
    Ok(Tag {
        id: row.try_get("id")?,
        label: row.try_get("label")?,
        description: row.try_get("description")?,
        color: row.try_get("color")?,
        created_at: row.try_get("created_at")?,
        owner_id: row.try_get("owner_id")?,
    })
}

#[async_trait]
impl TagsRepo for SqlxTagsRepo {
    async fn create(&self, tag: &Tag) -> Result<Tag, sqlx::Error> {
        let created = Tag {
            id: uuid::Uuid::new_v4().to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            ..tag.clone()
        };
        sqlx::query(
            "INSERT INTO tags (id, label, description, color, created_at, owner_id) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&created.id)
        .bind(&created.label)
        .bind(&created.description)
        .bind(&created.color)
        .bind(&created.created_at)
        .bind(created.owner_id.clone())
        .execute(&self.pool)
        .await?;
        Ok(created)
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<Tag>, sqlx::Error> {
        let sql = format!("SELECT {TAG_COLUMNS} FROM tags WHERE id = $1");
        let row = sqlx::query(AssertSqlSafe(sql))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(row_to_tag).transpose()
    }

    async fn find_by_label(
        &self,
        owner_id: Option<&str>,
        label: &str,
    ) -> Result<Option<Tag>, sqlx::Error> {
        let sql = match owner_id {
            Some(_) => format!(
                "SELECT {TAG_COLUMNS} FROM tags WHERE LOWER(label) = LOWER($1) AND owner_id = $2"
            ),
            None => format!(
                "SELECT {TAG_COLUMNS} FROM tags WHERE LOWER(label) = LOWER($1) AND owner_id IS NULL"
            ),
        };
        let mut q = sqlx::query(AssertSqlSafe(sql)).bind(label);
        if let Some(owner) = owner_id {
            q = q.bind(owner);
        }
        let row = q.fetch_optional(&self.pool).await?;
        row.as_ref().map(row_to_tag).transpose()
    }

    async fn list_visible(&self, owner_ids: Option<&[String]>) -> Result<Vec<Tag>, sqlx::Error> {
        let mut qb =
            sqlx::QueryBuilder::<sqlx::Any>::new(format!("SELECT {TAG_COLUMNS} FROM tags"));
        if let Some(owner_ids) = owner_ids {
            qb.push(" WHERE owner_id IS NULL");
            if !owner_ids.is_empty() {
                qb.push(" OR owner_id IN (");
                let mut sep = qb.separated(", ");
                for id in owner_ids {
                    sep.push_bind(id.clone());
                }
                qb.push(")");
            }
        }
        qb.push(" ORDER BY label");
        let rows = qb.build().fetch_all(&self.pool).await?;
        rows.iter().map(row_to_tag).collect()
    }

    async fn disown(&self, owner_id: &str) -> Result<u64, sqlx::Error> {
        let res = sqlx::query("UPDATE tags SET owner_id = NULL WHERE owner_id = $1")
            .bind(owner_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected())
    }

    async fn update(&self, tag: &Tag) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE tags SET label = $2, description = $3, color = $4 WHERE id = $1")
            .bind(&tag.id)
            .bind(&tag.label)
            .bind(&tag.description)
            .bind(&tag.color)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<bool, sqlx::Error> {
        // Explicit cleanup: do not rely on the driver enforcing the cascade.
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM transaction_tags WHERE tag_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        let res = sqlx::query("DELETE FROM tags WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(res.rows_affected() > 0)
    }

    async fn assignments_for_accounts(
        &self,
        account_ids: &[String],
    ) -> Result<Vec<(String, String)>, sqlx::Error> {
        if account_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut qb = sqlx::QueryBuilder::<sqlx::Any>::new(
            "SELECT tt.transaction_id, tt.tag_id FROM transaction_tags tt \
             JOIN transactions t ON t.id = tt.transaction_id WHERE t.account_id IN (",
        );
        let mut sep = qb.separated(", ");
        for id in account_ids {
            sep.push_bind(id.clone());
        }
        qb.push(")");
        let rows = qb.build().fetch_all(&self.pool).await?;
        rows.iter()
            .map(|r| Ok((r.try_get("transaction_id")?, r.try_get("tag_id")?)))
            .collect()
    }

    async fn set_for_transaction(
        &self,
        transaction_id: &str,
        tag_ids: &[String],
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM transaction_tags WHERE transaction_id = $1")
            .bind(transaction_id)
            .execute(&mut *tx)
            .await?;
        for tag_id in tag_ids {
            sqlx::query("INSERT INTO transaction_tags (transaction_id, tag_id) VALUES ($1, $2)")
                .bind(transaction_id)
                .bind(tag_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await
    }

    async fn bulk_change(
        &self,
        transaction_ids: &[String],
        add: &[String],
        remove: &[String],
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        for tid in transaction_ids {
            for tag_id in remove {
                sqlx::query(
                    "DELETE FROM transaction_tags WHERE transaction_id = $1 AND tag_id = $2",
                )
                .bind(tid)
                .bind(tag_id)
                .execute(&mut *tx)
                .await?;
            }
            for tag_id in add {
                // Insert-if-absent, portable across SQLite and MariaDB.
                sqlx::query(
                    "INSERT INTO transaction_tags (transaction_id, tag_id) \
                     SELECT $1, $2 WHERE NOT EXISTS \
                       (SELECT 1 FROM transaction_tags WHERE transaction_id = $1 AND tag_id = $2)",
                )
                .bind(tid)
                .bind(tag_id)
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await
    }
}
