//! Tag use-cases: per-user tag CRUD (like categories: owned by the creator,
//! shared when ownerless) and assigning tags to transactions,
//! permission-checked on the transaction's account.

use std::collections::BTreeSet;

use pfm_domain::{can_see, can_write, Account, Role};
use pfm_repo::{
    AccountsRepo, SharesRepo, SqlxAccountsRepo, SqlxAuthRepo, SqlxSharesRepo, SqlxTagsRepo,
    SqlxTransactionsRepo, Storage, Tag, TagsRepo, TransactionsRepo, UsersRepo,
};
use thiserror::Error;

use crate::ownership::{can_modify, Scope, Visibility};

#[derive(Debug, Error)]
pub enum TagError {
    #[error("tag not found")]
    NotFound,
    #[error("a tag with this label already exists")]
    Duplicate,
    #[error("forbidden")]
    Forbidden,
    #[error("validation error: {0}")]
    Validation(String),
    #[error("internal error: {0}")]
    Internal(String),
}

fn internal(e: impl std::fmt::Display) -> TagError {
    TagError::Internal(e.to_string())
}

const MAX_LABEL: usize = 64;
const MAX_DESCRIPTION: usize = 500;

/// Input for creating or updating a tag (`None` keeps the current value on update).
#[derive(Default)]
pub struct TagCmd {
    pub label: Option<String>,
    pub description: Option<String>,
    pub color: Option<String>,
}

/// A tag as seen by one user, with how often it is used on accounts they can see.
pub struct TagItem {
    pub tag: Tag,
    pub uses: i64,
    /// Whether this user may edit / delete it.
    pub editable: bool,
}

pub struct TagService {
    visibility: Visibility,
    tags: SqlxTagsRepo,
    transactions: SqlxTransactionsRepo,
    accounts: SqlxAccountsRepo,
    users: SqlxAuthRepo,
    shares: SqlxSharesRepo,
}

impl TagService {
    pub fn new(storage: &Storage) -> Self {
        Self {
            visibility: Visibility::new(storage),
            tags: SqlxTagsRepo::new(storage),
            transactions: SqlxTransactionsRepo::new(storage),
            accounts: SqlxAccountsRepo::new(storage),
            users: SqlxAuthRepo::new(storage),
            shares: SqlxSharesRepo::new(storage),
        }
    }

    fn clean(cmd: &TagCmd, base: Option<&Tag>) -> Result<Tag, TagError> {
        let label = cmd
            .label
            .as_deref()
            .map(str::trim)
            .map(String::from)
            .or_else(|| base.map(|t| t.label.clone()))
            .unwrap_or_default();
        if label.is_empty() {
            return Err(TagError::Validation("label must not be empty".into()));
        }
        if label.chars().count() > MAX_LABEL {
            return Err(TagError::Validation(format!(
                "label longer than {MAX_LABEL}"
            )));
        }
        let description = cmd
            .description
            .as_deref()
            .map(|s| s.trim().to_string())
            .or_else(|| base.map(|t| t.description.clone()))
            .unwrap_or_default();
        if description.chars().count() > MAX_DESCRIPTION {
            return Err(TagError::Validation(format!(
                "description longer than {MAX_DESCRIPTION}"
            )));
        }
        let color = cmd
            .color
            .clone()
            .or_else(|| base.map(|t| t.color.clone()))
            .unwrap_or_default();
        let hex = color.len() == 7
            && color.starts_with('#')
            && color[1..].chars().all(|c| c.is_ascii_hexdigit());
        if !color.is_empty() && !hex {
            return Err(TagError::Validation("color must be #rrggbb".into()));
        }
        Ok(Tag {
            id: base.map(|t| t.id.clone()).unwrap_or_default(),
            label,
            description,
            color,
            created_at: base.map(|t| t.created_at.clone()).unwrap_or_default(),
            owner_id: base.and_then(|t| t.owner_id.clone()),
        })
    }

    async fn ensure_label_free(
        &self,
        owner_id: Option<&str>,
        label: &str,
        except_id: &str,
    ) -> Result<(), TagError> {
        match self
            .tags
            .find_by_label(owner_id, label)
            .await
            .map_err(internal)?
        {
            Some(t) if t.id != except_id => Err(TagError::Duplicate),
            _ => Ok(()),
        }
    }

    /// Load a tag the actor can see (others read as not found) and whether
    /// they may modify it.
    async fn load_for(&self, actor_id: &str, id: &str) -> Result<(Tag, bool), TagError> {
        let tag = self
            .tags
            .get_by_id(id)
            .await
            .map_err(internal)?
            .ok_or(TagError::NotFound)?;
        let scope = self.visibility.scope(actor_id).await.map_err(internal)?;
        if !scope.sees(tag.owner_id.as_deref()) {
            return Err(TagError::NotFound);
        }
        let admin = self.visibility.is_admin(actor_id).await.map_err(internal)?;
        let editable = can_modify(actor_id, admin, tag.owner_id.as_deref());
        Ok((tag, editable))
    }

    pub async fn create(&self, actor_id: &str, cmd: &TagCmd) -> Result<Tag, TagError> {
        let mut tag = Self::clean(cmd, None)?;
        tag.owner_id = Some(actor_id.to_string());
        self.ensure_label_free(Some(actor_id), &tag.label, "")
            .await?;
        self.tags.create(&tag).await.map_err(internal)
    }

    pub async fn update(&self, actor_id: &str, id: &str, cmd: &TagCmd) -> Result<Tag, TagError> {
        let (current, editable) = self.load_for(actor_id, id).await?;
        if !editable {
            return Err(TagError::Forbidden);
        }
        let tag = Self::clean(cmd, Some(&current))?;
        self.ensure_label_free(current.owner_id.as_deref(), &tag.label, id)
            .await?;
        self.tags.update(&tag).await.map_err(internal)?;
        Ok(tag)
    }

    pub async fn delete(&self, actor_id: &str, id: &str) -> Result<(), TagError> {
        let (_, editable) = self.load_for(actor_id, id).await?;
        if !editable {
            return Err(TagError::Forbidden);
        }
        if self.tags.delete(id).await.map_err(internal)? {
            Ok(())
        } else {
            Err(TagError::NotFound)
        }
    }

    /// The tags the actor can see, each with the number of transactions using
    /// it on accounts the actor can see.
    pub async fn list(&self, actor_id: &str) -> Result<Vec<TagItem>, TagError> {
        let admin = self.visibility.is_admin(actor_id).await.map_err(internal)?;
        let scope = self
            .visibility
            .listing_scope(actor_id)
            .await
            .map_err(internal)?;
        let tags = match &scope {
            Scope::All => self.tags.list_visible(None).await,
            Scope::Owners(ids) => self.tags.list_visible(Some(ids)).await,
        }
        .map_err(internal)?;

        let mut uses: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        for (_, tag_id) in self.assignments(actor_id).await? {
            *uses.entry(tag_id).or_default() += 1;
        }
        Ok(tags
            .into_iter()
            .map(|tag| {
                let editable = can_modify(actor_id, admin, tag.owner_id.as_deref());
                let uses = uses.get(&tag.id).copied().unwrap_or(0);
                TagItem {
                    tag,
                    uses,
                    editable,
                }
            })
            .collect())
    }

    async fn is_admin(&self, actor_id: &str) -> Result<(Role, bool), TagError> {
        let actor = self
            .users
            .get_by_id(actor_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| TagError::Internal("actor not found".into()))?;
        Ok((actor.role, actor.role == Role::Admin))
    }

    /// `(transaction_id, tag_id)` for every transaction on an account the actor can see.
    pub async fn assignments(&self, actor_id: &str) -> Result<Vec<(String, String)>, TagError> {
        let (_, is_admin) = self.is_admin(actor_id).await?;
        let ids: Vec<String> = self
            .accounts
            .list_visible(actor_id, is_admin)
            .await
            .map_err(internal)?
            .into_iter()
            .map(|a| a.id)
            .collect();
        self.tags
            .assignments_for_accounts(&ids)
            .await
            .map_err(internal)
    }

    async fn check_write(
        &self,
        actor_id: &str,
        role: Role,
        account: &Account,
    ) -> Result<(), TagError> {
        let level = self
            .shares
            .get(&account.id, actor_id)
            .await
            .map_err(internal)?
            .map(|e| e.level);
        // `can_see` is implied by any share; writing needs `can_write`.
        if can_see(role, actor_id, &account.owner_id, level.is_some())
            && can_write(role, actor_id, &account.owner_id, level)
        {
            Ok(())
        } else {
            Err(TagError::Forbidden)
        }
    }

    /// Verify the actor may write every listed transaction and that all tag ids exist.
    async fn check_targets(
        &self,
        actor_id: &str,
        transaction_ids: &[String],
        tag_ids: &[String],
    ) -> Result<(), TagError> {
        let (role, _) = self.is_admin(actor_id).await?;
        let mut seen_accounts = BTreeSet::new();
        for id in transaction_ids {
            let tx = self
                .transactions
                .get_by_id(id)
                .await
                .map_err(internal)?
                .ok_or(TagError::NotFound)?;
            if seen_accounts.insert(tx.account_id.clone()) {
                let account = self
                    .accounts
                    .get_by_id(&tx.account_id)
                    .await
                    .map_err(internal)?
                    .ok_or_else(|| TagError::Internal("account not found".into()))?;
                self.check_write(actor_id, role, &account).await?;
            }
        }
        // Tags must exist *and* be ones the actor can see: otherwise another
        // user's private tag could be attached by guessing its id.
        let scope = self.visibility.scope(actor_id).await.map_err(internal)?;
        for tag_id in tag_ids {
            let tag = self
                .tags
                .get_by_id(tag_id)
                .await
                .map_err(internal)?
                .ok_or(TagError::NotFound)?;
            if !scope.sees(tag.owner_id.as_deref()) {
                return Err(TagError::NotFound);
            }
        }
        Ok(())
    }

    /// Replace the tag set of one transaction.
    pub async fn set_for_transaction(
        &self,
        actor_id: &str,
        transaction_id: &str,
        tag_ids: &[String],
    ) -> Result<(), TagError> {
        let unique: Vec<String> = tag_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        self.check_targets(actor_id, &[transaction_id.to_string()], &unique)
            .await?;
        self.tags
            .set_for_transaction(transaction_id, &unique)
            .await
            .map_err(internal)
    }

    /// Add/remove tags on a selection, all-or-nothing on permissions.
    pub async fn bulk_change(
        &self,
        actor_id: &str,
        transaction_ids: &[String],
        add: &[String],
        remove: &[String],
    ) -> Result<(), TagError> {
        if transaction_ids.is_empty() {
            return Err(TagError::Validation("no ids provided".into()));
        }
        let all_tags: Vec<String> = add.iter().chain(remove).cloned().collect();
        self.check_targets(actor_id, transaction_ids, &all_tags)
            .await?;
        self.tags
            .bulk_change(transaction_ids, add, remove)
            .await
            .map_err(internal)
    }
}
