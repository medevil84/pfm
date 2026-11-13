//! Account use-cases: CRUD with type-specific validation and permission checks.

use chrono::NaiveDate;
use pfm_domain::{can_see, can_write, Account, AccountKind, Role};
use pfm_repo::{
    AccountsRepo, AuditRepo, SharesRepo, SqlxAccountsRepo, SqlxAuthRepo, SqlxSharesRepo, Storage,
    UsersRepo,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AccountError {
    #[error("account not found")]
    NotFound,
    #[error("forbidden")]
    Forbidden,
    #[error("validation error: {0}")]
    Validation(String),
    #[error("account is referenced as a primary account by another account")]
    InUse,
    #[error("internal error: {0}")]
    Internal(String),
}

/// Input for creating a new account.
pub struct AccountCreateCmd {
    pub name: String,
    pub kind: AccountKind,
    pub icon: String,
    pub note: Option<String>,
    pub iban: Option<String>,
    pub currency: Option<String>,
    pub start_date: NaiveDate,
    pub start_balance_cents: i64,
    pub limit_cents: Option<i64>,
    pub charge_day: Option<i32>,
    pub closing_day: Option<i32>,
    pub primary_account_id: Option<String>,
    pub rate_pct_bps: Option<i64>,
    pub goal_cents: Option<i64>,
    pub min_balance_cents: Option<i64>,
}

/// Input for updating an existing account.
///
/// All fields are `Option`; `None` means "keep the current value".
pub struct AccountUpdateCmd {
    pub name: Option<String>,
    pub icon: Option<String>,
    pub note: Option<String>,
    pub iban: Option<String>,
    pub limit_cents: Option<i64>,
    pub charge_day: Option<i32>,
    pub closing_day: Option<i32>,
    pub primary_account_id: Option<String>,
    pub rate_pct_bps: Option<i64>,
    pub goal_cents: Option<i64>,
    pub min_balance_cents: Option<i64>,
    pub active: Option<bool>,
}

pub struct AccountService {
    accounts: SqlxAccountsRepo,
    users: SqlxAuthRepo,
    shares: SqlxSharesRepo,
}

impl AccountService {
    pub fn new(storage: &Storage) -> Self {
        Self {
            accounts: SqlxAccountsRepo::new(storage),
            users: SqlxAuthRepo::new(storage),
            shares: SqlxSharesRepo::new(storage),
        }
    }

    async fn is_admin(&self, user_id: &str) -> bool {
        self.users
            .get_by_id(user_id)
            .await
            .ok()
            .flatten()
            .is_some_and(|u| u.role == Role::Admin)
    }

    async fn check_access(
        &self,
        actor_id: &str,
        account: &Account,
        write: bool,
    ) -> Result<bool, AccountError> {
        if account.owner_id == actor_id {
            return Ok(true);
        }

        let actor = self
            .users
            .get_by_id(actor_id)
            .await
            .map_err(|e| AccountError::Internal(e.to_string()))?
            .ok_or_else(|| AccountError::Internal("actor not found".into()))?;

        if actor.role == Role::Admin {
            return Ok(true);
        }

        if !write {
            // Read: any share (read or write) grants visibility.
            let has_share = self
                .shares
                .get(&account.id, actor_id)
                .await
                .map_err(|e| AccountError::Internal(e.to_string()))?
                .is_some();
            Ok(can_see(actor.role, actor_id, &account.owner_id, has_share))
        } else {
            // Write: only a Write-level share grants mutation access.
            let share_level = self
                .shares
                .get(&account.id, actor_id)
                .await
                .map_err(|e| AccountError::Internal(e.to_string()))?
                .map(|e| e.level);
            Ok(can_write(
                actor.role,
                actor_id,
                &account.owner_id,
                share_level,
            ))
        }
    }

    /// A card / deposit settles onto its `primary_account_id`, so the actor
    /// must be allowed to write to that account: otherwise anyone who knows a
    /// foreign account id could make the system post rows onto it.
    async fn ensure_backing_writable(
        &self,
        actor_id: &str,
        primary_account_id: &str,
        own_id: Option<&str>,
    ) -> Result<(), AccountError> {
        if own_id == Some(primary_account_id) {
            return Err(AccountError::Validation(
                "an account cannot be its own primary account".into(),
            ));
        }
        let backing = self
            .accounts
            .get_by_id(primary_account_id)
            .await
            .map_err(|e| AccountError::Internal(e.to_string()))?
            .ok_or_else(|| AccountError::Validation("primary account not found".into()))?;
        if !self.check_access(actor_id, &backing, true).await? {
            return Err(AccountError::Forbidden);
        }
        Ok(())
    }

    fn validate_charge_day(day: Option<i32>) -> Result<(), AccountError> {
        if let Some(d) = day {
            if !(1..=28).contains(&d) {
                return Err(AccountError::Validation(
                    "charge_day must be between 1 and 28".into(),
                ));
            }
        }
        Ok(())
    }

    fn validate_closing_day(day: Option<i32>) -> Result<(), AccountError> {
        if let Some(d) = day {
            if !(1..=28).contains(&d) {
                return Err(AccountError::Validation(
                    "closing_day must be between 1 and 28".into(),
                ));
            }
        }
        Ok(())
    }

    fn validate_create(kind: &AccountKind, cmd: &AccountCreateCmd) -> Result<(), AccountError> {
        Self::validate_charge_day(cmd.charge_day)?;
        Self::validate_closing_day(cmd.closing_day)?;
        match kind {
            AccountKind::Card => {
                if cmd.limit_cents.is_none() {
                    return Err(AccountError::Validation(
                        "card accounts require limit_cents".into(),
                    ));
                }
                if cmd.charge_day.is_none() {
                    return Err(AccountError::Validation(
                        "card accounts require charge_day".into(),
                    ));
                }
            }
            AccountKind::Deposit => {
                if cmd.rate_pct_bps.is_none() {
                    return Err(AccountError::Validation(
                        "deposit accounts require rate_pct_bps".into(),
                    ));
                }
                if cmd.primary_account_id.is_none() {
                    return Err(AccountError::Validation(
                        "deposit accounts require primary_account_id".into(),
                    ));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn validate_update(
        kind: &AccountKind,
        current: &Account,
        cmd: &AccountUpdateCmd,
    ) -> Result<(), AccountError> {
        let charge = cmd.charge_day.or(current.charge_day);
        Self::validate_charge_day(charge)?;
        let closing = cmd.closing_day.or(current.closing_day);
        Self::validate_closing_day(closing)?;

        let limit = cmd.limit_cents.or(current.limit_cents);
        let rate = cmd.rate_pct_bps.or(current.rate_pct_bps);
        let primary = cmd
            .primary_account_id
            .as_ref()
            .or(current.primary_account_id.as_ref());

        match kind {
            AccountKind::Card => {
                if limit.is_none() {
                    return Err(AccountError::Validation(
                        "card accounts require limit_cents".into(),
                    ));
                }
                if charge.is_none() {
                    return Err(AccountError::Validation(
                        "card accounts require charge_day".into(),
                    ));
                }
            }
            AccountKind::Deposit => {
                if rate.is_none() {
                    return Err(AccountError::Validation(
                        "deposit accounts require rate_pct_bps".into(),
                    ));
                }
                if primary.is_none() {
                    return Err(AccountError::Validation(
                        "deposit accounts require primary_account_id".into(),
                    ));
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Create a new account owned by `actor_id`.
    pub async fn create(
        &self,
        actor_id: &str,
        cmd: &AccountCreateCmd,
    ) -> Result<Account, AccountError> {
        Self::validate_create(&cmd.kind, cmd)?;
        if let Some(primary) = cmd.primary_account_id.as_deref() {
            self.ensure_backing_writable(actor_id, primary, None)
                .await?;
        }

        let currency = match &cmd.currency {
            Some(c) => c.clone(),
            None => {
                let user = self
                    .users
                    .get_by_id(actor_id)
                    .await
                    .map_err(|e| AccountError::Internal(e.to_string()))?
                    .ok_or_else(|| AccountError::Internal("owner not found".into()))?;
                user.default_currency
            }
        };

        let account = Account {
            id: String::new(),
            owner_id: actor_id.to_string(),
            name: cmd.name.clone(),
            kind: cmd.kind,
            icon: cmd.icon.clone(),
            note: cmd.note.clone(),
            iban: cmd.iban.clone(),
            currency,
            start_date: cmd.start_date,
            start_balance_cents: cmd.start_balance_cents,
            limit_cents: cmd.limit_cents,
            charge_day: cmd.charge_day,
            // Cards default the closing day to the charge day when omitted.
            closing_day: cmd.closing_day.or(if cmd.kind == AccountKind::Card {
                cmd.charge_day
            } else {
                None
            }),
            primary_account_id: cmd.primary_account_id.clone(),
            rate_pct_bps: cmd.rate_pct_bps,
            goal_cents: cmd.goal_cents,
            min_balance_cents: cmd.min_balance_cents,
            active: true,
            created_at: String::new(),
            sort_order: 0, // repo computes the real value (max + 1)
            last_settled_date: NaiveDate::from_ymd_opt(1900, 1, 1).unwrap(),
        };

        self.accounts
            .create(actor_id, &account)
            .await
            .map_err(|e| AccountError::Internal(e.to_string()))
    }

    /// Fetch a single account, enforcing owner-or-admin-or-share access.
    pub async fn get(&self, actor_id: &str, account_id: &str) -> Result<Account, AccountError> {
        let account = self
            .accounts
            .get_by_id(account_id)
            .await
            .map_err(|e| AccountError::Internal(e.to_string()))?
            .ok_or(AccountError::NotFound)?;

        if !self.check_access(actor_id, &account, false).await? {
            return Err(AccountError::Forbidden);
        }
        Ok(account)
    }

    /// List the actor's visible accounts (owned + shared).
    pub async fn list(&self, actor_id: &str) -> Result<Vec<Account>, AccountError> {
        let is_admin = self.is_admin(actor_id).await;
        self.accounts
            .list_visible(actor_id, is_admin)
            .await
            .map_err(|e| AccountError::Internal(e.to_string()))
    }

    /// Update mutable fields on an existing account.
    pub async fn update(
        &self,
        actor_id: &str,
        account_id: &str,
        cmd: &AccountUpdateCmd,
    ) -> Result<Account, AccountError> {
        let account = self
            .accounts
            .get_by_id(account_id)
            .await
            .map_err(|e| AccountError::Internal(e.to_string()))?
            .ok_or(AccountError::NotFound)?;

        if !self.check_access(actor_id, &account, true).await? {
            return Err(AccountError::Forbidden);
        }

        Self::validate_update(&account.kind, &account, cmd)?;
        if let Some(primary) = cmd.primary_account_id.as_deref() {
            if account.primary_account_id.as_deref() != Some(primary) {
                self.ensure_backing_writable(actor_id, primary, Some(&account.id))
                    .await?;
            }
        }

        let updated = Account {
            id: account.id.clone(),
            owner_id: account.owner_id.clone(),
            name: cmd.name.clone().unwrap_or_else(|| account.name.clone()),
            kind: account.kind,
            icon: cmd.icon.clone().unwrap_or_else(|| account.icon.clone()),
            note: cmd.note.clone().or(account.note.clone()),
            iban: cmd.iban.clone().or(account.iban.clone()),
            currency: account.currency.clone(),
            sort_order: account.sort_order,
            start_date: account.start_date,
            start_balance_cents: account.start_balance_cents,
            limit_cents: cmd.limit_cents.or(account.limit_cents),
            charge_day: cmd.charge_day.or(account.charge_day),
            closing_day: cmd.closing_day.or(account.closing_day),
            primary_account_id: cmd
                .primary_account_id
                .clone()
                .or(account.primary_account_id.clone()),
            rate_pct_bps: cmd.rate_pct_bps.or(account.rate_pct_bps),
            goal_cents: cmd.goal_cents.or(account.goal_cents),
            min_balance_cents: cmd.min_balance_cents.or(account.min_balance_cents),
            active: cmd.active.unwrap_or(account.active),
            created_at: account.created_at.clone(),
            last_settled_date: account.last_settled_date,
        };

        self.accounts
            .update(&updated)
            .await
            .map_err(|e| AccountError::Internal(e.to_string()))?;
        Ok(updated)
    }

    /// Reorder the caller's owned accounts by assigning `sort_order = index`
    /// for each id in the supplied slice. Accounts that appear in `ids` but are
    /// not owned by `actor_id` are silently skipped; shared or foreign accounts
    /// keep whatever sort_order their owner last set.
    pub async fn reorder(&self, actor_id: &str, ids: &[String]) -> Result<(), AccountError> {
        let owned = self
            .accounts
            .list_by_owner(actor_id)
            .await
            .map_err(|e| AccountError::Internal(e.to_string()))?;
        let owned_ids: std::collections::HashSet<&str> =
            owned.iter().map(|a| a.id.as_str()).collect();

        let updates: Vec<(String, i64)> = ids
            .iter()
            .enumerate()
            .filter(|(_, id)| owned_ids.contains(id.as_str()))
            .map(|(i, id)| (id.clone(), i as i64))
            .collect();

        self.accounts
            .reorder_batch(&updates)
            .await
            .map_err(|e| AccountError::Internal(e.to_string()))
    }

    /// Delete an account. Blocked if another account references it as `primary_account_id`.
    pub async fn delete(&self, actor_id: &str, account_id: &str) -> Result<(), AccountError> {
        let account = self
            .accounts
            .get_by_id(account_id)
            .await
            .map_err(|e| AccountError::Internal(e.to_string()))?
            .ok_or(AccountError::NotFound)?;

        if !self.check_access(actor_id, &account, true).await? {
            return Err(AccountError::Forbidden);
        }

        let referenced = self
            .accounts
            .referenced_as_primary(account_id)
            .await
            .map_err(|e| AccountError::Internal(e.to_string()))?;
        if referenced {
            return Err(AccountError::InUse);
        }

        self.accounts
            .delete(account_id)
            .await
            .map_err(|e| AccountError::Internal(e.to_string()))?;

        // Audit: account self-deleted by owner.
        let event = pfm_domain::AuditEvent::new(pfm_domain::AuditAction::AccountSelfDeleted)
            .with_actor(actor_id)
            .with_entity("account", account_id);
        let _ = self.users.log(&event).await;

        Ok(())
    }
}
