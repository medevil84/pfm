//! Transaction use-cases: CRUD with permission checks and the register
//! (per-day balance view) endpoint.

use chrono::{Duration, NaiveDate};
use pfm_domain::{
    build_register, can_see, can_write, fee_horizon_in, generate_fees_in, Account, AccountKind,
    FeePeriod, PeriodMetrics, RegisterResult, Role, Transaction, TransactionRow,
};
use pfm_repo::{
    AccountRulesRepo, AccountsRepo, PrefsRepo, RegisterFilters, SharesRepo, SqlxAccountRulesRepo,
    SqlxAccountsRepo, SqlxAuthRepo, SqlxSharesRepo, SqlxTransactionsRepo, Storage,
    TransactionsRepo, UsersRepo,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TransactionError {
    #[error("transaction not found")]
    NotFound,
    #[error("account not found")]
    AccountNotFound,
    #[error("forbidden")]
    Forbidden,
    #[error("validation error: {0}")]
    Validation(String),
    #[error("internal error: {0}")]
    Internal(String),
}

/// Input for creating a new transaction.
pub struct TransactionCreateCmd {
    pub account_id: String,
    pub date: NaiveDate,
    pub cents: i64,
    pub description: Option<String>,
    pub category_id: Option<String>,
    pub note: Option<String>,
    /// Client-generated idempotency key (UUID). When present and already
    /// seen by the server, the existing row is returned instead of
    /// creating a duplicate.
    pub client_id: Option<String>,
}

/// Input for updating an existing transaction. All fields are `Option`;
/// `None` means "keep the current value".
pub struct TransactionUpdateCmd {
    pub date: Option<NaiveDate>,
    pub cents: Option<i64>,
    pub description: Option<String>,
    pub category_id: Option<String>,
    pub note: Option<String>,
    /// Move the movement to another account. `None` (or the current account)
    /// keeps it where it is. Refused for movements where
    /// [`Transaction::can_change_account`] is false.
    pub account_id: Option<String>,
}

/// One autocomplete proposal: a description together with the amount and
/// category of its most recent use.
pub struct DescriptionSuggestion {
    pub description: String,
    pub cents: i64,
    pub category_id: Option<String>,
    pub count: u32,
}

pub struct TransactionService {
    transactions: SqlxTransactionsRepo,
    accounts: SqlxAccountsRepo,
    rules: SqlxAccountRulesRepo,
    users: SqlxAuthRepo,
    shares: SqlxSharesRepo,
    cards: crate::card::CardService,
    holidays: crate::holidays::HolidayService,
    category_access: crate::CategoryAccess,
}

impl TransactionService {
    pub fn new(storage: &Storage) -> Self {
        Self {
            transactions: SqlxTransactionsRepo::new(storage),
            accounts: SqlxAccountsRepo::new(storage),
            rules: SqlxAccountRulesRepo::new(storage),
            users: SqlxAuthRepo::new(storage),
            shares: SqlxSharesRepo::new(storage),
            cards: crate::card::CardService::new(storage),
            holidays: crate::holidays::HolidayService::new(storage),
            category_access: crate::CategoryAccess::new(storage),
        }
    }

    /// A `category_id` sent by a client must exist and be one the actor may use
    /// (own, shared, or a collaborator's); anything else is a validation error
    /// instead of a database foreign-key failure or a cross-user reference.
    async fn ensure_category_usable(
        &self,
        actor_id: &str,
        category_id: &str,
    ) -> Result<(), TransactionError> {
        self.category_access
            .ensure_usable(actor_id, category_id)
            .await
            .map_err(|e| match e {
                crate::CategoryAccessError::NotFound => {
                    TransactionError::Validation("unknown category".into())
                }
                crate::CategoryAccessError::Internal(m) => TransactionError::Internal(m),
            })
    }

    async fn check_access(
        &self,
        actor_id: &str,
        account: &Account,
        write: bool,
    ) -> Result<bool, TransactionError> {
        if account.owner_id == actor_id {
            return Ok(true);
        }

        let actor = self
            .users
            .get_by_id(actor_id)
            .await
            .map_err(|e| TransactionError::Internal(e.to_string()))?
            .ok_or_else(|| TransactionError::Internal("actor not found".into()))?;

        if actor.role == Role::Admin {
            return Ok(true);
        }

        if !write {
            let has_share = self
                .shares
                .get(&account.id, actor_id)
                .await
                .map_err(|e| TransactionError::Internal(e.to_string()))?
                .is_some();
            Ok(can_see(actor.role, actor_id, &account.owner_id, has_share))
        } else {
            let share_level = self
                .shares
                .get(&account.id, actor_id)
                .await
                .map_err(|e| TransactionError::Internal(e.to_string()))?
                .map(|e| e.level);
            Ok(can_write(
                actor.role,
                actor_id,
                &account.owner_id,
                share_level,
            ))
        }
    }

    async fn get_account_checked(
        &self,
        actor_id: &str,
        account_id: &str,
        write: bool,
    ) -> Result<Account, TransactionError> {
        let account = self
            .accounts
            .get_by_id(account_id)
            .await
            .map_err(|e| TransactionError::Internal(e.to_string()))?
            .ok_or(TransactionError::AccountNotFound)?;

        if !self.check_access(actor_id, &account, write).await? {
            return Err(TransactionError::Forbidden);
        }
        Ok(account)
    }

    /// If `account` is a credit card, trigger a delta settlement for the
    /// affected dates. Silently ignored for non-card accounts and for
    /// settlement rows (which have a `cc:` pair_id — they are the *output*
    /// of settlement, not the input).
    async fn settle_card_if_needed(
        &self,
        account: &Account,
        affected_dates: &[NaiveDate],
        tx_pair_id: &Option<String>,
    ) -> Result<(), TransactionError> {
        // Settlement rows (pair_id = "cc:…") are the output of settlement;
        // modifying or deleting them should not trigger a re-settlement.
        if let Some(pid) = tx_pair_id {
            if pid.starts_with("cc:") {
                return Ok(());
            }
        }

        if account.kind != AccountKind::Card {
            return Ok(());
        }
        let charge_day = account
            .charge_day
            .ok_or_else(|| TransactionError::Validation("card has no charge_day".into()))?;
        let closing_day = account.closing_day.unwrap_or(charge_day);
        let backing = account
            .primary_account_id
            .as_ref()
            .ok_or_else(|| TransactionError::Validation("card has no backing account".into()))?;

        self.cards
            .settle_delta(
                &account.id,
                crate::card::CardCycleDays {
                    closing_day,
                    charge_day,
                },
                account.start_date,
                account.last_settled_date,
                backing,
                affected_dates,
            )
            .await
            .map_err(|e| TransactionError::Internal(e.to_string()))?;
        Ok(())
    }

    /// Create a new transaction on the given account.
    ///
    /// When `cmd.client_id` is `Some`, the method performs an idempotency
    /// check: if a transaction with that `client_id` already exists, the
    /// existing row is returned (no new row is created). This enables
    /// safe offline sync: the client generates a UUID before sending and
    /// retries are harmless.
    pub async fn create(
        &self,
        actor_id: &str,
        cmd: &TransactionCreateCmd,
    ) -> Result<Transaction, TransactionError> {
        let _account = self
            .get_account_checked(actor_id, &cmd.account_id, true)
            .await?;

        if cmd.cents == 0 {
            return Err(TransactionError::Validation(
                "cents must be non-zero".into(),
            ));
        }
        if let Some(cat) = cmd.category_id.as_deref() {
            self.ensure_category_usable(actor_id, cat).await?;
        }

        // Idempotency: if the client sent a known client_id, return the
        // existing row instead of creating a duplicate.
        if let Some(ref cid) = cmd.client_id {
            if let Some(existing) = self
                .transactions
                .find_by_client_id(cid)
                .await
                .map_err(|e| TransactionError::Internal(e.to_string()))?
            {
                // Idempotent retry only when the stored row is the same
                // request (same account). `client_id` is unique database-wide:
                // handing back a row that lives on a different account would
                // disclose it to whoever guesses / learns its id.
                if existing.account_id != cmd.account_id {
                    return Err(TransactionError::Validation(
                        "client_id already in use".into(),
                    ));
                }
                return Ok(existing);
            }
        }

        // Use the client-provided id for idempotency, or generate a random UUID
        // for legacy clients that don't send one.
        let client_id = cmd
            .client_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let tx = Transaction {
            id: String::new(),
            client_id,
            account_id: cmd.account_id.clone(),
            date: cmd.date,
            cents: cmd.cents,
            description: cmd.description.clone(),
            category_id: cmd.category_id.clone(),
            note: cmd.note.clone(),
            pair_id: None,
            rec_id: None,
            rule_id: None,
            override_: None,
            computed: None,
            vacct_id: None,
            import_hash: None,
            created_by: actor_id.to_string(),
            created_at: String::new(),
            updated_at: String::new(),
        };

        let created = self
            .transactions
            .create(&tx)
            .await
            .map_err(|e| TransactionError::Internal(e.to_string()))?;

        // Trigger card settlement delta if the account is a credit card.
        if let Some(ref acc) = self
            .accounts
            .get_by_id(&cmd.account_id)
            .await
            .map_err(|e| TransactionError::Internal(e.to_string()))?
        {
            let dates = vec![cmd.date];
            let _ = self
                .settle_card_if_needed(acc, &dates, &created.pair_id)
                .await;
        }

        Ok(created)
    }

    /// Fetch a single transaction, enforcing account-level access.
    pub async fn get(&self, actor_id: &str, tx_id: &str) -> Result<Transaction, TransactionError> {
        let tx = self
            .transactions
            .get_by_id(tx_id)
            .await
            .map_err(|e| TransactionError::Internal(e.to_string()))?
            .ok_or(TransactionError::NotFound)?;

        let _account = self
            .get_account_checked(actor_id, &tx.account_id, false)
            .await?;
        Ok(tx)
    }

    /// Fetch all transactions sharing a `pair_id` (both legs of a settlement
    /// or transfer). Access is checked on the first leg's account.
    pub async fn get_by_pair(
        &self,
        actor_id: &str,
        pair_id: &str,
    ) -> Result<Vec<Transaction>, TransactionError> {
        let legs = self
            .transactions
            .get_transfer_legs(pair_id)
            .await
            .map_err(|e| TransactionError::Internal(e.to_string()))?;

        if legs.is_empty() {
            return Err(TransactionError::NotFound);
        }

        // The caller must be able to read every leg's account: returning the
        // counterpart leg would otherwise expose an account the caller cannot
        // see. Legs of accounts that are not visible are reported as missing.
        for tx in &legs {
            if self
                .get_account_checked(actor_id, &tx.account_id, false)
                .await
                .is_err()
            {
                return Err(TransactionError::NotFound);
            }
        }

        Ok(legs)
    }

    /// Description autocomplete over the actor's own and shared accounts
    /// (admins get no cross-user peek: suggestions are personal habits).
    /// Groups recent rows by description (case-insensitive); amount and
    /// category come from the most recent use, most frequent first.
    /// `sign` (`< 0` outflow, `> 0` inflow) keeps only rows of that direction.
    pub async fn suggest_descriptions(
        &self,
        actor_id: &str,
        text: &str,
        sign: Option<i8>,
        limit: usize,
    ) -> Result<Vec<DescriptionSuggestion>, TransactionError> {
        let ids: Vec<String> = self
            .accounts
            .list_visible(actor_id, false)
            .await
            .map_err(|e| TransactionError::Internal(e.to_string()))?
            .into_iter()
            .map(|a| a.id)
            .collect();

        let rows = self
            .transactions
            .recent_by_description(&ids, text, 500)
            .await
            .map_err(|e| TransactionError::Internal(e.to_string()))?;

        // Rows arrive newest first, so the first row seen for a description
        // is its most recent use.
        let mut groups: Vec<(String, DescriptionSuggestion)> = Vec::new();
        for tx in rows {
            if let Some(sg) = sign {
                if (sg < 0 && tx.cents >= 0) || (sg > 0 && tx.cents <= 0) {
                    continue;
                }
            }
            let Some(desc) = tx.description.as_deref().map(str::trim) else {
                continue;
            };
            if desc.is_empty() {
                continue;
            }
            let key = desc.to_lowercase();
            if let Some((_, g)) = groups.iter_mut().find(|(k, _)| *k == key) {
                g.count += 1;
            } else {
                groups.push((
                    key,
                    DescriptionSuggestion {
                        description: desc.to_string(),
                        cents: tx.cents,
                        category_id: tx.category_id.clone(),
                        count: 1,
                    },
                ));
            }
        }
        let mut out: Vec<DescriptionSuggestion> = groups.into_iter().map(|(_, g)| g).collect();
        // Stable sort keeps recency order among equal counts.
        out.sort_by_key(|a| std::cmp::Reverse(a.count));
        out.truncate(limit);
        Ok(out)
    }

    /// Update mutable fields on an existing transaction.
    pub async fn update(
        &self,
        actor_id: &str,
        tx_id: &str,
        cmd: &TransactionUpdateCmd,
    ) -> Result<Transaction, TransactionError> {
        let tx = self
            .transactions
            .get_by_id(tx_id)
            .await
            .map_err(|e| TransactionError::Internal(e.to_string()))?
            .ok_or(TransactionError::NotFound)?;

        let account = self
            .get_account_checked(actor_id, &tx.account_id, true)
            .await?;

        if let Some(cat) = cmd.category_id.as_deref() {
            if tx.category_id.as_deref() != Some(cat) {
                self.ensure_category_usable(actor_id, cat).await?;
            }
        }

        // Moving to another account: the row must be a plain manual movement
        // and the actor must be able to write on the destination as well.
        let target_account = match cmd.account_id.as_deref() {
            Some(dest) if dest != tx.account_id => {
                if !tx.can_change_account() {
                    return Err(TransactionError::Validation(
                        "account cannot be changed for transfers, recurring, rule-generated, \
                         virtual-account or imported movements"
                            .into(),
                    ));
                }
                Some(self.get_account_checked(actor_id, dest, true).await?)
            }
            _ => None,
        };
        let new_date = cmd.date.unwrap_or(tx.date);
        let cents = cmd.cents.unwrap_or(tx.cents);
        if cents == 0 {
            return Err(TransactionError::Validation(
                "cents must be non-zero".into(),
            ));
        }

        let updated = Transaction {
            id: tx.id.clone(),
            client_id: tx.client_id.clone(),
            account_id: target_account
                .as_ref()
                .map_or_else(|| tx.account_id.clone(), |a| a.id.clone()),
            date: new_date,
            cents,
            description: cmd.description.clone().or(tx.description.clone()),
            category_id: cmd.category_id.clone().or(tx.category_id.clone()),
            note: cmd.note.clone().or(tx.note.clone()),
            pair_id: tx.pair_id.clone(),
            rec_id: tx.rec_id.clone(),
            rule_id: tx.rule_id.clone(),
            override_: tx.override_,
            computed: tx.computed,
            vacct_id: tx.vacct_id.clone(),
            import_hash: tx.import_hash.clone(),
            created_by: tx.created_by.clone(),
            created_at: tx.created_at.clone(),
            updated_at: tx.updated_at.clone(),
        };

        self.transactions
            .update(&updated)
            .await
            .map_err(|e| TransactionError::Internal(e.to_string()))?;

        // Trigger card settlement delta: affected dates are the new date always,
        // plus the old date if it changed (so the old cycle is also re-settled).
        if let Some(dest) = &target_account {
            // Moved: the origin account loses the row on its old date, the
            // destination gains it on the new date. Either may be a card.
            let _ = self
                .settle_card_if_needed(&account, &[tx.date], &tx.pair_id)
                .await;
            let _ = self
                .settle_card_if_needed(dest, &[new_date], &tx.pair_id)
                .await;
        } else {
            let affected_dates: Vec<NaiveDate> = if new_date != tx.date {
                vec![tx.date, new_date]
            } else {
                vec![tx.date]
            };
            let _ = self
                .settle_card_if_needed(&account, &affected_dates, &tx.pair_id)
                .await;
        }

        Ok(updated)
    }

    /// Delete a transaction by id.
    pub async fn delete(&self, actor_id: &str, tx_id: &str) -> Result<(), TransactionError> {
        let tx = self
            .transactions
            .get_by_id(tx_id)
            .await
            .map_err(|e| TransactionError::Internal(e.to_string()))?
            .ok_or(TransactionError::NotFound)?;

        let account = self
            .get_account_checked(actor_id, &tx.account_id, true)
            .await?;

        self.transactions
            .delete(tx_id)
            .await
            .map_err(|e| TransactionError::Internal(e.to_string()))?;

        // Trigger card settlement delta for the deleted transaction's date.
        let _ = self
            .settle_card_if_needed(&account, &[tx.date], &tx.pair_id)
            .await;

        Ok(())
    }

    /// Build the per-day register for an account over a date range.
    ///
    /// Fetches filtered transaction rows from the repo, computes the running
    /// balance in the service layer (to avoid window-function portability
    /// issues with the `Any` driver), and delegates to
    /// [`build_register`] for empty-day filling and summary.
    pub async fn register(
        &self,
        actor_id: &str,
        account_id: &str,
        from: NaiveDate,
        to: NaiveDate,
        filters: &RegisterFilters,
    ) -> Result<RegisterResult, TransactionError> {
        let account = self
            .get_account_checked(actor_id, account_id, false)
            .await?;

        let prefs = self
            .users
            .get(actor_id)
            .await
            .map_err(|e| TransactionError::Internal(e.to_string()))?;

        let pre_sum = self
            .transactions
            .sum_cents_before_filtered(account_id, from, filters.vacct_included.as_deref())
            .await
            .map_err(|e| TransactionError::Internal(e.to_string()))?;
        let opening_balance = account.start_balance_cents + pre_sum;

        let today = chrono::Utc::now().date_naive();

        let rows = self
            .transactions
            .list_for_register(account_id, from, to, filters)
            .await
            .map_err(|e| TransactionError::Internal(e.to_string()))?;

        // Merge real transactions with projected fee rows for future dates.
        let mut all_txs: Vec<Transaction> = rows;

        // A card's rule oneri live on the backing account (applied per cycle by
        // the settlement engine), so they are never projected onto the card
        // register itself.
        if to > today && account.kind != AccountKind::Card {
            let proj_from = (today + Duration::days(1)).max(from);
            let account_rules = self
                .rules
                .list_for_account(account_id)
                .await
                .map_err(|e| TransactionError::Internal(e.to_string()))?;

            // Balance just before the future projection window — used as the
            // metric base for balance-conditional fee rules.
            let balance_before_future: i64 = opening_balance
                + all_txs
                    .iter()
                    .filter(|t| t.date < proj_from)
                    .map(|t| t.cents)
                    .sum::<i64>();

            let metrics = |_p: &FeePeriod| PeriodMetrics {
                avg_balance_cents: balance_before_future,
                end_balance_cents: balance_before_future,
                ..Default::default()
            };

            let calendar = self.holidays.for_user(&account.owner_id).await;
            for rule in account_rules.iter().filter(|r| r.active) {
                let fee_rule = rule.fee_rule();
                if fee_horizon_in(&fee_rule, proj_from, to, &calendar).is_none() {
                    continue;
                }
                for fee_row in generate_fees_in(&fee_rule, proj_from, to, &calendar, metrics) {
                    all_txs.push(Transaction {
                        id: String::new(),
                        client_id: String::new(),
                        account_id: account_id.to_string(),
                        date: fee_row.date,
                        cents: fee_row.amount_cents,
                        description: Some(rule.name.clone()),
                        category_id: Some(rule.category_id.clone()),
                        note: None,
                        pair_id: None,
                        rec_id: None,
                        rule_id: Some(rule.id.clone()),
                        override_: None,
                        computed: Some(true),
                        vacct_id: None,
                        import_hash: None,
                        created_by: String::new(),
                        created_at: String::new(),
                        updated_at: String::new(),
                    });
                }
            }

            // Keep rows ordered by date so the running balance is correct.
            all_txs.sort_by(|a, b| a.date.cmp(&b.date).then(a.id.cmp(&b.id)));
        }

        let mut running = opening_balance;
        let mut rows_with_balance: Vec<TransactionRow> = Vec::with_capacity(all_txs.len());
        for tx in all_txs {
            running += tx.cents;
            rows_with_balance.push(TransactionRow {
                running_balance_cents: running,
                tx,
            });
        }

        Ok(build_register(
            opening_balance,
            rows_with_balance,
            from,
            to,
            today,
            prefs.show_future,
        ))
    }
}
