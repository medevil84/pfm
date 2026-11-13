//! Category repository: a trait (so the service depends on an abstraction) with
//! a single SQLx-`Any` implementation serving both SQLite and MariaDB.
//!
//! The `key` slug is generated from the name at creation time and is never
//! rewritten on update, so movement references by category stay stable. User-
//! created categories are always non-fallback (`is_fallback = 0`); only the
//! seeded rows are fallbacks.

use async_trait::async_trait;
use pfm_domain::{Category, CategoryKind};
use sqlx::any::AnyRow;
use sqlx::{AnyPool, AssertSqlSafe, Row};

use crate::Storage;

/// Read/write access to categories.
#[async_trait]
pub trait CategoriesRepo: Send + Sync {
    /// Insert a new category. The repository generates the id, the `key` slug
    /// (from the name), and `created_at`. The caller-supplied `category`
    /// carries `name`, `emoji`, `kind` and `budget_cents`; `is_fallback` is
    /// forced to `false`. The persisted row is returned.
    async fn create(&self, category: &Category) -> Result<Category, sqlx::Error>;

    /// Look up a category by id.
    async fn get_by_id(&self, id: &str) -> Result<Option<Category>, sqlx::Error>;

    /// All categories (fallback and user-created), ordered by kind then name.
    async fn list(&self) -> Result<Vec<Category>, sqlx::Error>;

    /// Replace a category's mutable fields by id. Only `name`, `emoji` and
    /// `budget_cents` change; `key` and `kind` are fixed at creation and are
    /// never rewritten.
    async fn update(&self, category: &Category) -> Result<(), sqlx::Error>;

    /// Delete a category by id. Returns `true` if a row was removed.
    async fn delete(&self, id: &str) -> Result<bool, sqlx::Error>;

    /// Shared categories (no owner) plus those owned by any of `owner_ids`,
    /// ordered by kind then name.
    async fn list_visible(&self, owner_ids: &[String]) -> Result<Vec<Category>, sqlx::Error>;

    /// How many recurrences (as category or linked category) and fee rules use
    /// the category. Deleting a category cascades to those rows, so callers
    /// refuse the delete while this is non-zero.
    async fn count_references(&self, id: &str) -> Result<i64, sqlx::Error>;

    /// Make every category owned by `owner_id` shared (used when that user is
    /// deleted, so the rows their transactions point to do not become
    /// unmanageable). Returns the number of rows changed.
    async fn disown(&self, owner_id: &str) -> Result<u64, sqlx::Error>;
}

/// A single SQLx-`Any` implementation of the categories repository, bound to
/// one pool.
#[derive(Clone)]
pub struct SqlxCategoriesRepo {
    pool: AnyPool,
}

impl SqlxCategoriesRepo {
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

/// Derive a stable slug from a name: lowercased, non-alphanumeric runs
/// collapsed to a single `-`, leading/trailing `-` trimmed.
pub fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_dash = true;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "cat".to_string()
    } else {
        out
    }
}

/// The set of `categories` columns selected by every category-reading query.
const CATEGORY_COLUMNS: &str =
    "id, key, name, emoji, kind, budget_cents, is_fallback, created_at, owner_id";

#[async_trait]
impl CategoriesRepo for SqlxCategoriesRepo {
    async fn create(&self, category: &Category) -> Result<Category, sqlx::Error> {
        let id = uuid::Uuid::new_v4().to_string();
        // `key` is globally unique. Owned categories carry a short owner suffix
        // so two users can both have a "Groceries"; shared ones keep the bare
        // slug.
        let mut key = slugify(&category.name);
        if let Some(owner) = category.owner_id.as_deref() {
            let short: String = owner
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(8)
                .collect();
            key = format!("{key}-{short}");
        }
        let created_at = now_rfc3339();

        sqlx::query(
            "INSERT INTO categories \
             (id, key, name, emoji, kind, budget_cents, is_fallback, created_at, owner_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(&id)
        .bind(&key)
        .bind(&category.name)
        .bind(&category.emoji)
        .bind(category.kind.as_str())
        .bind(category.budget_cents)
        .bind(int_bool(false))
        .bind(&created_at)
        .bind(category.owner_id.clone())
        .execute(&self.pool)
        .await?;

        Ok(Category {
            id,
            key,
            name: category.name.clone(),
            emoji: category.emoji.clone(),
            kind: category.kind,
            budget_cents: category.budget_cents,
            is_fallback: false,
            created_at,
            owner_id: category.owner_id.clone(),
        })
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<Category>, sqlx::Error> {
        let sql = format!("SELECT {CATEGORY_COLUMNS} FROM categories WHERE id = $1");
        let row = sqlx::query(AssertSqlSafe(sql))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_category).transpose()
    }

    async fn list(&self) -> Result<Vec<Category>, sqlx::Error> {
        let sql = format!("SELECT {CATEGORY_COLUMNS} FROM categories ORDER BY kind, name");
        let rows = sqlx::query(AssertSqlSafe(sql))
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_category).collect()
    }

    async fn list_visible(&self, owner_ids: &[String]) -> Result<Vec<Category>, sqlx::Error> {
        let mut qb = sqlx::QueryBuilder::<sqlx::Any>::new(format!(
            "SELECT {CATEGORY_COLUMNS} FROM categories WHERE owner_id IS NULL"
        ));
        if !owner_ids.is_empty() {
            qb.push(" OR owner_id IN (");
            let mut sep = qb.separated(", ");
            for id in owner_ids {
                sep.push_bind(id.clone());
            }
            qb.push(")");
        }
        qb.push(" ORDER BY kind, name");
        let rows = qb.build().fetch_all(&self.pool).await?;
        rows.into_iter().map(row_to_category).collect()
    }

    async fn count_references(&self, id: &str) -> Result<i64, sqlx::Error> {
        let row = sqlx::query(
            "SELECT \
               (SELECT COUNT(*) FROM recurrences WHERE category_id = $1 OR linked_category_id = $1) \
             + (SELECT COUNT(*) FROM account_rules WHERE category_id = $1)",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?;
        row.try_get::<i64, _>(0)
    }

    async fn disown(&self, owner_id: &str) -> Result<u64, sqlx::Error> {
        let res = sqlx::query("UPDATE categories SET owner_id = NULL WHERE owner_id = $1")
            .bind(owner_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected())
    }

    async fn update(&self, category: &Category) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE categories SET name = $2, emoji = $3, budget_cents = $4 WHERE id = $1")
            .bind(&category.id)
            .bind(&category.name)
            .bind(&category.emoji)
            .bind(category.budget_cents)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM categories WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

/// Map a `categories` row into a [`Category`], decoding the `INTEGER` boolean
/// and the kind string.
fn row_to_category(row: AnyRow) -> Result<Category, sqlx::Error> {
    let kind: String = row.try_get("kind")?;
    let is_fallback: i64 = row.try_get("is_fallback")?;
    Ok(Category {
        id: row.try_get("id")?,
        key: row.try_get("key")?,
        name: row.try_get("name")?,
        emoji: row.try_get("emoji")?,
        kind: CategoryKind::parse(&kind).unwrap_or(CategoryKind::Out),
        budget_cents: row.try_get("budget_cents")?,
        is_fallback: is_fallback != 0,
        created_at: row.try_get("created_at")?,
        owner_id: row.try_get("owner_id")?,
    })
}
