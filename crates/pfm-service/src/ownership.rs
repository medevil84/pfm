//! Who may see and use categories and tags.
//!
//! Both belong to the user who created them (`owner_id`); rows without an owner
//! (the seeded fallbacks and everything that predates per-user ownership) are
//! shared. A user sees the shared rows, their own, and those of the people they
//! collaborate with through account sharing — transactions on a shared account
//! carry whatever categories and tags either side attached. Administrators see
//! everything.

use pfm_domain::Role;
use pfm_repo::{
    CategoriesRepo, SqlxAuthRepo, SqlxCategoriesRepo, SqlxSharesRepo, Storage, UsersRepo,
};

/// Which owners' rows a user can see.
#[derive(Debug, Clone)]
pub enum Scope {
    /// An administrator: everything.
    All,
    /// Shared rows plus those owned by one of these users (the user themself
    /// and their collaborators).
    Owners(Vec<String>),
}

impl Scope {
    pub fn sees(&self, owner_id: Option<&str>) -> bool {
        match (self, owner_id) {
            (Scope::All, _) => true,
            (_, None) => true,
            (Scope::Owners(ids), Some(o)) => ids.iter().any(|i| i == o),
        }
    }
}

/// May `actor` change (rename / delete) a row owned by `owner_id`?
/// Owners can; administrators can; shared rows only administrators.
pub fn can_modify(actor_id: &str, actor_is_admin: bool, owner_id: Option<&str>) -> bool {
    actor_is_admin || owner_id == Some(actor_id)
}

#[derive(Clone)]
pub struct Visibility {
    users: SqlxAuthRepo,
    shares: SqlxSharesRepo,
}

impl Visibility {
    pub fn new(storage: &Storage) -> Self {
        Self {
            users: SqlxAuthRepo::new(storage),
            shares: SqlxSharesRepo::new(storage),
        }
    }

    pub async fn is_admin(&self, actor_id: &str) -> Result<bool, String> {
        Ok(self
            .users
            .get_by_id(actor_id)
            .await
            .map_err(|e| e.to_string())?
            .map(|u| u.role == Role::Admin)
            .unwrap_or(false))
    }

    /// What the user may *use or open by id*: administrators reach everything.
    pub async fn scope(&self, actor_id: &str) -> Result<Scope, String> {
        if self.is_admin(actor_id).await? {
            return Ok(Scope::All);
        }
        self.listing_scope(actor_id).await
    }

    /// What the user sees when *browsing* (pickers, management pages): their
    /// own, shared and collaborators' rows — for administrators too, so their
    /// lists are not flooded with every user's private items. Admin authority
    /// still applies to by-id operations.
    pub async fn listing_scope(&self, actor_id: &str) -> Result<Scope, String> {
        let mut ids = self
            .shares
            .related_user_ids(actor_id)
            .await
            .map_err(|e| e.to_string())?;
        ids.push(actor_id.to_string());
        Ok(Scope::Owners(ids))
    }
}

/// Why a category cannot be used.
#[derive(Debug)]
pub enum CategoryAccessError {
    /// Missing, or owned by someone the actor does not collaborate with
    /// (indistinguishable on purpose).
    NotFound,
    Internal(String),
}

/// Checks that a category id supplied by a client is one the actor may use.
/// Every service that stores a `category_id` goes through this: without it a
/// request could attach any user's category (a database foreign-key error at
/// best, a cross-user reference at worst).
#[derive(Clone)]
pub struct CategoryAccess {
    categories: SqlxCategoriesRepo,
    visibility: Visibility,
}

impl CategoryAccess {
    pub fn new(storage: &Storage) -> Self {
        Self {
            categories: SqlxCategoriesRepo::new(storage),
            visibility: Visibility::new(storage),
        }
    }

    pub async fn ensure_usable(
        &self,
        actor_id: &str,
        category_id: &str,
    ) -> Result<(), CategoryAccessError> {
        let category = self
            .categories
            .get_by_id(category_id)
            .await
            .map_err(|e| CategoryAccessError::Internal(e.to_string()))?
            .ok_or(CategoryAccessError::NotFound)?;
        let scope = self
            .visibility
            .scope(actor_id)
            .await
            .map_err(CategoryAccessError::Internal)?;
        if scope.sees(category.owner_id.as_deref()) {
            Ok(())
        } else {
            Err(CategoryAccessError::NotFound)
        }
    }
}
