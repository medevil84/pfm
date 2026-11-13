//! Category use-cases: per-user ownership, CRUD with kind-immutability and
//! fallback-delete rejection.
//!
//! A category belongs to the user who created it (`owner_id`); rows without an
//! owner are shared and administrator-managed. Reads are limited to the
//! categories the actor can see (their own, shared, and those of the people they
//! share accounts with); writes to the owner or an administrator.

use pfm_domain::{Category, CategoryKind};
use pfm_repo::{slugify, CategoriesRepo, SqlxCategoriesRepo, Storage};
use thiserror::Error;

use crate::ownership::{can_modify, Visibility};

#[derive(Debug, Error)]
pub enum CategoryError {
    #[error("category not found")]
    NotFound,
    #[error("validation error: {0}")]
    Validation(String),
    #[error("cannot delete a fallback category")]
    FallbackProtected,
    #[error("forbidden")]
    Forbidden,
    #[error("a category with this name already exists")]
    Duplicate,
    #[error("category is used by recurrences or fee rules")]
    InUse,
    #[error("internal error: {0}")]
    Internal(String),
}

/// Input for creating a new category.
pub struct CategoryCreateCmd {
    pub name: String,
    pub emoji: String,
    pub kind: CategoryKind,
    pub budget_cents: Option<i64>,
}

/// Input for updating an existing category.
///
/// All fields are `Option`; `None` means "keep the current value".
pub struct CategoryUpdateCmd {
    pub name: Option<String>,
    pub emoji: Option<String>,
    pub budget_cents: Option<i64>,
}

/// A category as seen by one user.
pub struct CategoryItem {
    pub category: Category,
    /// Whether this user may rename / delete it.
    pub editable: bool,
}

fn is_unique_violation(e: &impl std::fmt::Display) -> bool {
    let msg = e.to_string();
    msg.contains("UNIQUE") || msg.contains("Duplicate entry")
}

fn internal(e: impl std::fmt::Display) -> CategoryError {
    CategoryError::Internal(e.to_string())
}

pub struct CategoryService {
    categories: SqlxCategoriesRepo,
    visibility: Visibility,
}

impl CategoryService {
    pub fn new(storage: &Storage) -> Self {
        Self {
            categories: SqlxCategoriesRepo::new(storage),
            visibility: Visibility::new(storage),
        }
    }

    /// Create a category owned by `actor_id`. The stable `key` slug is derived
    /// from the name at creation time only.
    pub async fn create(
        &self,
        actor_id: &str,
        cmd: &CategoryCreateCmd,
    ) -> Result<CategoryItem, CategoryError> {
        if cmd.name.trim().is_empty() {
            return Err(CategoryError::Validation("name must not be empty".into()));
        }
        if cmd.emoji.trim().is_empty() {
            return Err(CategoryError::Validation("emoji must not be empty".into()));
        }

        let key = slugify(&cmd.name);
        if key.is_empty() {
            return Err(CategoryError::Validation(
                "name must contain at least one alphanumeric character".into(),
            ));
        }

        let category = Category {
            id: String::new(),
            key,
            name: cmd.name.clone(),
            emoji: cmd.emoji.clone(),
            kind: cmd.kind,
            budget_cents: cmd.budget_cents,
            is_fallback: false,
            created_at: String::new(),
            owner_id: Some(actor_id.to_string()),
        };

        let created = self.categories.create(&category).await.map_err(|e| {
            if is_unique_violation(&e) {
                CategoryError::Duplicate
            } else {
                internal(e)
            }
        })?;
        Ok(CategoryItem {
            category: created,
            editable: true,
        })
    }

    /// Fetch a category the actor can see (others read as not found).
    pub async fn get(
        &self,
        actor_id: &str,
        category_id: &str,
    ) -> Result<CategoryItem, CategoryError> {
        let category = self
            .categories
            .get_by_id(category_id)
            .await
            .map_err(internal)?
            .ok_or(CategoryError::NotFound)?;
        let scope = self.visibility.scope(actor_id).await.map_err(internal)?;
        if !scope.sees(category.owner_id.as_deref()) {
            return Err(CategoryError::NotFound);
        }
        let admin = self.visibility.is_admin(actor_id).await.map_err(internal)?;
        let editable = can_modify(actor_id, admin, category.owner_id.as_deref());
        Ok(CategoryItem { category, editable })
    }

    /// The categories the actor can see.
    pub async fn list(&self, actor_id: &str) -> Result<Vec<CategoryItem>, CategoryError> {
        use crate::ownership::Scope;
        let admin = self.visibility.is_admin(actor_id).await.map_err(internal)?;
        let scope = self
            .visibility
            .listing_scope(actor_id)
            .await
            .map_err(internal)?;
        let rows = match &scope {
            Scope::All => self.categories.list().await,
            Scope::Owners(ids) => self.categories.list_visible(ids).await,
        }
        .map_err(internal)?;
        Ok(rows
            .into_iter()
            .map(|category| {
                let editable = can_modify(actor_id, admin, category.owner_id.as_deref());
                CategoryItem { category, editable }
            })
            .collect())
    }

    /// Update mutable fields on an existing category (owner or admin only).
    ///
    /// `kind` and `is_fallback` are immutable and ignored.
    pub async fn update(
        &self,
        actor_id: &str,
        category_id: &str,
        cmd: &CategoryUpdateCmd,
    ) -> Result<CategoryItem, CategoryError> {
        let current = self.get(actor_id, category_id).await?;
        if !current.editable {
            return Err(CategoryError::Forbidden);
        }
        let category = current.category;

        let name = cmd.name.clone().unwrap_or_else(|| category.name.clone());
        let emoji = cmd.emoji.clone().unwrap_or_else(|| category.emoji.clone());
        if name.trim().is_empty() {
            return Err(CategoryError::Validation("name must not be empty".into()));
        }

        let updated = Category {
            id: category.id.clone(),
            key: category.key.clone(),
            name,
            emoji,
            kind: category.kind,
            budget_cents: cmd.budget_cents.or(category.budget_cents),
            is_fallback: category.is_fallback,
            created_at: category.created_at.clone(),
            owner_id: category.owner_id.clone(),
        };

        self.categories.update(&updated).await.map_err(internal)?;
        Ok(CategoryItem {
            category: updated,
            editable: true,
        })
    }

    /// Delete a category (owner or admin only). Rejected for the seeded
    /// fallbacks, and while recurrences or fee rules still use it: the database
    /// would cascade the delete to them.
    pub async fn delete(&self, actor_id: &str, category_id: &str) -> Result<(), CategoryError> {
        let current = self.get(actor_id, category_id).await?;
        if current.category.is_fallback {
            return Err(CategoryError::FallbackProtected);
        }
        if !current.editable {
            return Err(CategoryError::Forbidden);
        }
        if self
            .categories
            .count_references(category_id)
            .await
            .map_err(internal)?
            > 0
        {
            return Err(CategoryError::InUse);
        }
        self.categories
            .delete(category_id)
            .await
            .map_err(internal)?;
        Ok(())
    }
}
