//! Transaction repository: a trait (so the service depends on an abstraction)
//! with a single SQLx-`Any` implementation serving both SQLite and MariaDB.
//!
//! The `list_for_register` method builds a dynamic query with optional filters
//! (category, type, min/max amount, text search) and returns rows sorted by
//! (date, id). The running balance is computed by the caller (service layer)
//! via a simple cumulative sum, avoiding window-function portability issues
//! with the `Any` driver.

use async_trait::async_trait;
use chrono::NaiveDate;
use pfm_domain::{AccountRule, FeeRow, RealOccurrence, Transaction};
use sqlx::any::AnyRow;
use sqlx::{Acquire, AnyPool, AssertSqlSafe, Row};

use crate::Storage;

/// Filter parameters for the register query. All fields are `None` to disable.
#[derive(Debug, Clone, Default)]
pub struct RegisterFilters {
    /// Only include transactions with this category id.
    pub category_id: Option<String>,
    /// Filter by type: `"in"` (positive cents), `"out"` (negative cents), or `None` for both.
    pub tx_type: Option<String>,
    /// Minimum absolute amount in cents (inclusive).
    pub min_cents: Option<i64>,
    /// Maximum absolute amount in cents (inclusive).
    pub max_cents: Option<i64>,
    /// Case-insensitive text search over description and note.
    pub text: Option<String>,
    /// Piggy-bank (virtual account) id selection for the "available" view.
    ///
    /// `None` keeps the legacy behaviour (every row is shown). `Some(ids)`
    /// narrows the ledger to the rows the user still wants to see on this
    /// account: the non-allocated rows (`vacct_id IS NULL`), this account's own
    /// piggy rows (`vacct_id = account_id`, so a piggy bank's own register is
    /// always complete), and the allocation legs of the piggy banks in `ids`.
    pub vacct_included: Option<Vec<String>>,
}

/// Read/write access to transactions.
#[async_trait]
pub trait TransactionsRepo: Send + Sync {
    /// Insert a new transaction. The repository generates the id and
    /// timestamps. Returns the persisted row.
    async fn create(&self, tx: &Transaction) -> Result<Transaction, sqlx::Error>;

    /// Look up a transaction by id.
    async fn get_by_id(&self, id: &str) -> Result<Option<Transaction>, sqlx::Error>;

    /// Look up a transaction by its client-generated id (for idempotent
    /// offline sync). Returns the existing row if `client_id` was already
    /// seen, `None` otherwise.
    async fn find_by_client_id(&self, client_id: &str) -> Result<Option<Transaction>, sqlx::Error>;

    /// Replace a transaction's mutable fields by id.
    async fn update(&self, tx: &Transaction) -> Result<(), sqlx::Error>;

    /// Delete a transaction by id. Returns `true` if a row was removed.
    async fn delete(&self, id: &str) -> Result<bool, sqlx::Error>;

    /// List transactions for the register: all rows for the given account
    /// within the date range, optionally filtered, sorted by (date, id).
    async fn list_for_register(
        &self,
        account_id: &str,
        from: NaiveDate,
        to: NaiveDate,
        filters: &RegisterFilters,
    ) -> Result<Vec<Transaction>, sqlx::Error>;

    /// Most recent plain rows (no transfer legs, non-empty description) on the
    /// given accounts whose description contains `text` (case-insensitive,
    /// wildcards escaped), newest first. Feeds description autocomplete; the
    /// caller groups by description.
    async fn recent_by_description(
        &self,
        account_ids: &[String],
        text: &str,
        limit: i64,
    ) -> Result<Vec<Transaction>, sqlx::Error>;

    /// Sum of all cents for the account with date strictly before `date`.
    /// Used to compute the balance at the start of a register range.
    async fn sum_cents_before(&self, account_id: &str, date: NaiveDate)
        -> Result<i64, sqlx::Error>;

    /// Prefix-sum of cents for the account with date strictly before `date`,
    /// narrowed by the same piggy-bank selection the register applies (see
    /// [`RegisterFilters::vacct_included`]). This keeps the register's opening
    /// balance consistent with the rows it shows, so deselecting a piggy bank
    /// recomputes the running balance exactly as if that piggy never existed.
    /// `None` matches [`sum_cents_before`].
    async fn sum_cents_before_filtered(
        &self,
        account_id: &str,
        date: NaiveDate,
        vacct_included: Option<&[String]>,
    ) -> Result<i64, sqlx::Error>;

    /// Every row on the account (any date, any piggy allocation), sorted by
    /// (date, id). Feeds the two-balance (real vs. available) computation.
    async fn list_all_rows(&self, account_id: &str) -> Result<Vec<Transaction>, sqlx::Error>;

    /// Atomically create a transfer pair: two transactions sharing `pair_id`,
    /// one on `from_account_id` (negative cents) and one on `to_account_id`
    /// (positive cents). Returns both persisted rows.
    ///
    /// When the transfer touches a piggy bank (either leg), `vacct_id` is set
    /// on **both** legs to that piggy bank's id, so the allocation is hidden
    /// from the real balance and from any ledger that deselects the piggy.
    /// `None` for a transfer between two real accounts.
    /// `client_id` carries the **client's** idempotency key and is protected by
    /// a UNIQUE index (migration 0009). Only rows created on behalf of a client
    /// request may carry a client-supplied value; every row generated by the
    /// server (transfer legs, card settlements, deposit interest, rule rows,
    /// import rows) uses its own primary key as `client_id`, which is unique by
    /// construction. Multi-row writers therefore take **no** `client_id`
    /// argument: passing one shared value for N rows would violate the index and
    /// roll the whole batch back.
    #[allow(clippy::too_many_arguments)]
    async fn create_transfer_pair(
        &self,
        pair_id: &str,
        from_account_id: &str,
        to_account_id: &str,
        date: NaiveDate,
        cents: i64,
        description: &Option<String>,
        note: &Option<String>,
        created_by: &str,
        vacct_id: Option<&str>,
    ) -> Result<(Transaction, Transaction), sqlx::Error>;

    /// Atomically delete both legs of a transfer pair by `pair_id`.
    async fn delete_transfer_pair(&self, pair_id: &str) -> Result<(), sqlx::Error>;

    /// Fetch both legs of a transfer pair by `pair_id`.
    async fn get_transfer_legs(&self, pair_id: &str) -> Result<Vec<Transaction>, sqlx::Error>;

    /// List all transactions that belong to a transfer pair (non-null pair_id)
    /// within a date range, optionally filtered by source/destination account.
    async fn list_transfer_legs(
        &self,
        from: NaiveDate,
        to: NaiveDate,
        from_account_id: Option<&str>,
        to_account_id: Option<&str>,
    ) -> Result<Vec<Transaction>, sqlx::Error>;

    /// Atomically delete multiple transactions by id. Returns the number of
    /// rows actually deleted.
    async fn bulk_delete(&self, ids: &[String]) -> Result<u64, sqlx::Error>;

    /// Atomically set `category_id` on multiple transactions. Returns the
    /// number of rows updated.
    async fn bulk_recategorize(
        &self,
        ids: &[String],
        category_id: Option<&str>,
    ) -> Result<u64, sqlx::Error>;

    /// Real (confirmed / overridden) occurrences for a recurrence: the signed
    /// cents of every `override = true` transaction whose `rec_id` matches.
    /// Feeds the average-based amount computation.
    async fn real_occurrences_for_recurrence(
        &self,
        rec_id: &str,
    ) -> Result<Vec<RealOccurrence>, sqlx::Error>;

    /// Every transaction linked to a recurrence (`rec_id`), both legs of a
    /// transfer recurrence included, optionally restricted to `date >= from`,
    /// sorted by (date, id).
    async fn list_for_recurrence(
        &self,
        rec_id: &str,
        from: Option<NaiveDate>,
    ) -> Result<Vec<Transaction>, sqlx::Error>;

    /// Atomically set `cents` on several transactions (`(id, cents)` pairs).
    /// Returns the number of rows updated.
    async fn set_cents_bulk(&self, updates: &[(String, i64)]) -> Result<u64, sqlx::Error>;

    /// Sum of all cents for the account with date strictly before `date`,
    /// **excluding** rows produced by the rules engine (`computed = 1`). Used to
    /// compute the base balance that feeds the rules' average-balance metric,
    /// so a rule never sees its own (or any rule's) generated rows.
    async fn sum_noncomputed_cents_before(
        &self,
        account_id: &str,
        date: NaiveDate,
    ) -> Result<i64, sqlx::Error>;

    /// All non-engine rows for the account within `[from, to]`, sorted by
    /// (date, id). Rows with `computed = 1` are excluded, so the result is the
    /// manual / confirmed base ledger independent of any rule's output.
    async fn list_noncomputed(
        &self,
        account_id: &str,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<Transaction>, sqlx::Error>;

    /// Atomically regenerate the engine rows of one rule within `[from, to]`:
    /// in a single transaction it deletes that rule's `computed = 1` rows in the
    /// horizon and re-creates them from `rows`. Manual rows (`computed` null or
    /// `0`, or a different `rule_id`) are never touched.
    async fn regenerate_rule_rows(
        &self,
        rule: &AccountRule,
        created_by: &str,
        from: NaiveDate,
        to: NaiveDate,
        rows: &[FeeRow],
    ) -> Result<(), sqlx::Error>;

    /// Delete all engine (`computed = 1`) rows produced by the given rule on the
    /// given account, so removing a rule leaves no orphaned fee rows. Returns
    /// the number of rows removed. Manual rows and other rules' rows are untouched.
    async fn delete_rule_rows(&self, rule_id: &str, account_id: &str) -> Result<u64, sqlx::Error>;

    /// Raw (non-settlement) card movements on the account within the half-open
    /// window `[from, to_exclusive)`, sorted by (date, id). Settlement legs
    /// (rows carrying a `pair_id`) are excluded, so the result is exactly the
    /// raw ledger the end-of-cycle charge is computed from and a settlement is
    /// never re-billed on its own legs.
    /// Raw card movements billed by one settlement cycle: the half-open window
    /// `(after, closing]` (lower bound exclusive — that day closed the previous
    /// cycle — upper bound inclusive on the closing day). Engine-generated rows
    /// (`computed = 1`) are excluded so the card's reset credit reflects only the
    /// cardholder's real spend; rule-defined oneri are applied to the backing
    /// account by the settlement layer, never to the card.
    async fn list_card_cycle(
        &self,
        account_id: &str,
        after: NaiveDate,
        closing: NaiveDate,
    ) -> Result<Vec<Transaction>, sqlx::Error>;

    /// Sum of ALL cents on the account (raw movements and settlement legs
    /// alike). Added to the account's opening balance it is the account's
    /// current balance, which drives the card's remaining limit.
    async fn sum_all_cents(&self, account_id: &str) -> Result<i64, sqlx::Error>;

    /// Sum of raw (non-settlement) card movements with date strictly after
    /// `after_date` and up to `until_date` inclusive (future-dated movements
    /// are not yet part of the balance). Used to compute the card's outstanding
    /// (unbilled) balance for the remaining-limit calculation.
    async fn sum_raw_after(
        &self,
        account_id: &str,
        after_date: NaiveDate,
        until_date: NaiveDate,
    ) -> Result<i64, sqlx::Error>;

    /// The earliest raw (non-settlement) transaction date on the card.
    /// Used as an effective start date for cycle generation when the stored
    /// `start_date` is later than the oldest imported transaction.
    async fn min_raw_date(&self, account_id: &str) -> Result<Option<NaiveDate>, sqlx::Error>;

    /// The earliest non-engine (`computed` null/0) row on the account, any kind.
    /// Lets the fee rules reach back to ledger history that predates the
    /// account's stored `start_date`.
    async fn min_noncomputed_date(
        &self,
        account_id: &str,
    ) -> Result<Option<NaiveDate>, sqlx::Error>;

    /// Delete both legs of a card settlement pair by `pair_id`.
    async fn delete_card_charge_pair(&self, pair_id: &str) -> Result<(), sqlx::Error>;

    /// Write only the card's reset credit of a settlement (the cycle is closed
    /// but the backing account is not yet debited). Any backing leg of the same
    /// `pair_id` is removed, so the debit stays a forecast until it is due; an
    /// existing card leg keeps its date and only has its amount refreshed.
    async fn upsert_card_reset_leg(
        &self,
        pair_id: &str,
        card_account_id: &str,
        card_date: NaiveDate,
        card_cents: i64,
        description: &Option<String>,
        created_by: &str,
    ) -> Result<(), sqlx::Error>;

    /// Atomically (re)settle a card cycle as a single linked pair with legs on
    /// **different dates and amounts**:
    ///
    /// - the card is credited `card_cents` (positive leg) on `card_date` — the
    ///   accounting closing day, resetting the card's exposure to zero using the
    ///   cardholder's real spend only;
    /// - the backing account is debited `backing_cents` (negative leg) on
    ///   `backing_date` — the working-day-adjusted charge date. `backing_cents`
    ///   is the spend plus any rule-defined oneri (bollo, canoni), so the two
    ///   legs need not be equal.
    ///
    /// Any existing rows sharing `pair_id` are removed first, so re-running a
    /// settlement replaces it instead of stacking. Returns both persisted legs,
    /// primary (backing) leg first.
    #[allow(clippy::too_many_arguments)]
    async fn create_card_charge_pair(
        &self,
        pair_id: &str,
        primary_account_id: &str,
        card_account_id: &str,
        card_date: NaiveDate,
        backing_date: NaiveDate,
        card_cents: i64,
        backing_cents: i64,
        description: &Option<String>,
        created_by: &str,
    ) -> Result<(Transaction, Transaction), sqlx::Error>;

    /// Atomically credit deposit interest: a linked pair that credits
    /// `deposit_account_id` with `gross_cents` (interest earned) and credits
    /// `primary_account_id` with `net_cents` (net interest after withholding,
    /// transferred to the linked current account) on `date`. Both legs are
    /// positive (inflows from the bank). Any existing rows sharing `pair_id`
    /// are removed first, so re-consolidating the same period replaces the
    /// prior rows. Returns both persisted legs, deposit leg first.
    #[allow(clippy::too_many_arguments)]
    async fn create_deposit_interest_pair(
        &self,
        pair_id: &str,
        deposit_account_id: &str,
        primary_account_id: &str,
        date: NaiveDate,
        gross_cents: i64,
        net_cents: i64,
        description: &Option<String>,
        created_by: &str,
    ) -> Result<(Transaction, Transaction), sqlx::Error>;

    /// Atomically insert multiple import rows in a single transaction.
    /// Each row carries its pre-computed `import_hash` for idempotency.
    /// Returns the number of rows inserted.
    async fn batch_import(
        &self,
        account_id: &str,
        rows: &[ImportRowData],
        created_by: &str,
    ) -> Result<u64, sqlx::Error>;
}

/// A row of data to be inserted by [`TransactionsRepo::batch_import`].
#[derive(Debug, Clone)]
pub struct ImportRowData {
    pub date: NaiveDate,
    pub cents: i64,
    pub description: Option<String>,
    pub import_hash: String,
}

/// A single SQLx-`Any` implementation of the transactions repository, bound
/// to one pool.
#[derive(Clone)]
pub struct SqlxTransactionsRepo {
    pool: AnyPool,
}

impl SqlxTransactionsRepo {
    /// Build a repository handle from an existing [`Storage`] (shares its pool).
    pub fn new(storage: &Storage) -> Self {
        Self {
            pool: storage.pool().clone(),
        }
    }
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn date_iso(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

fn parse_date(s: &str) -> Result<NaiveDate, sqlx::Error> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|e| sqlx::Error::Decode(Box::new(e)))
}

const TX_COLUMNS: &str = "id, client_id, account_id, date, cents, description, category_id, \
      note, pair_id, rec_id, rule_id, override, computed, vacct_id, import_hash, \
      created_by, created_at, updated_at";

/// Append the piggy-bank visibility clause to a query for `account_id`.
///
/// When `vacct_included` is `None` no clause is emitted (legacy: show every
/// row). Otherwise only the rows the user still sees on this account are kept:
/// the non-allocated rows, this account's own piggy rows (so a piggy bank's
/// register is always complete even when it is deselected elsewhere), and the
/// allocation legs of the piggy banks in `vacct_included`.
fn push_vacct_clause(
    qb: &mut sqlx::QueryBuilder<sqlx::Any>,
    account_id: &str,
    vacct_included: Option<&[String]>,
) {
    match vacct_included {
        None => {}
        Some([]) => {
            qb.push(" AND (vacct_id IS NULL OR vacct_id = ")
                .push_bind(account_id.to_string())
                .push(")");
        }
        Some(ids) => {
            qb.push(" AND (vacct_id IS NULL OR vacct_id = ")
                .push_bind(account_id.to_string())
                .push(" OR vacct_id IN (");
            qb.push_values(ids, |mut values, id| {
                values.push_bind(id.to_string());
            });
            qb.push("))");
        }
    }
}

#[async_trait]
impl TransactionsRepo for SqlxTransactionsRepo {
    async fn create(&self, tx: &Transaction) -> Result<Transaction, sqlx::Error> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = now_rfc3339();

        sqlx::query(
            "INSERT INTO transactions \
             (id, client_id, account_id, date, cents, description, category_id, note, \
              pair_id, rec_id, rule_id, override, computed, vacct_id, import_hash, \
              created_by, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)",
        )
        .bind(&id)
        .bind(&tx.client_id)
        .bind(&tx.account_id)
        .bind(date_iso(tx.date))
        .bind(tx.cents)
        .bind(&tx.description)
        .bind(&tx.category_id)
        .bind(&tx.note)
        .bind(&tx.pair_id)
        .bind(&tx.rec_id)
        .bind(&tx.rule_id)
        .bind(tx.override_.map(|b| if b { 1i64 } else { 0 }))
        .bind(tx.computed.map(|b| if b { 1i64 } else { 0 }))
        .bind(&tx.vacct_id)
        .bind(&tx.import_hash)
        .bind(&tx.created_by)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        Ok(Transaction {
            id,
            created_at: now.clone(),
            updated_at: now,
            ..tx.clone()
        })
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<Transaction>, sqlx::Error> {
        let sql = format!("SELECT {TX_COLUMNS} FROM transactions WHERE id = $1");
        let row = sqlx::query(AssertSqlSafe(sql))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_transaction).transpose()
    }

    async fn find_by_client_id(&self, client_id: &str) -> Result<Option<Transaction>, sqlx::Error> {
        let sql = format!("SELECT {TX_COLUMNS} FROM transactions WHERE client_id = $1");
        let row = sqlx::query(AssertSqlSafe(sql))
            .bind(client_id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_transaction).transpose()
    }

    async fn update(&self, tx: &Transaction) -> Result<(), sqlx::Error> {
        let updated_at = now_rfc3339();
        sqlx::query(
            "UPDATE transactions SET \
             date = $2, cents = $3, description = $4, category_id = $5, note = $6, \
             updated_at = $7, account_id = $8 WHERE id = $1",
        )
        .bind(&tx.id)
        .bind(date_iso(tx.date))
        .bind(tx.cents)
        .bind(&tx.description)
        .bind(&tx.category_id)
        .bind(&tx.note)
        .bind(&updated_at)
        .bind(&tx.account_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM transactions WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn list_for_register(
        &self,
        account_id: &str,
        from: NaiveDate,
        to: NaiveDate,
        filters: &RegisterFilters,
    ) -> Result<Vec<Transaction>, sqlx::Error> {
        let mut qb = sqlx::QueryBuilder::<sqlx::Any>::new(format!(
            "SELECT {TX_COLUMNS} FROM transactions WHERE "
        ));
        qb.push("account_id = ").push_bind(account_id.to_string());
        qb.push(" AND date >= ").push_bind(date_iso(from));
        qb.push(" AND date <= ").push_bind(date_iso(to));

        if let Some(ref cat) = filters.category_id {
            qb.push(" AND category_id = ").push_bind(cat.clone());
        }

        match filters.tx_type.as_deref() {
            Some("in") => {
                qb.push(" AND cents > 0");
            }
            Some("out") => {
                qb.push(" AND cents < 0");
            }
            _ => {}
        }

        if let Some(min) = filters.min_cents {
            qb.push(" AND ABS(cents) >= ").push_bind(min);
        }
        if let Some(max) = filters.max_cents {
            qb.push(" AND ABS(cents) <= ").push_bind(max);
        }

        if let Some(ref text) = filters.text {
            let pattern = format!("%{text}%");
            qb.push(" AND (description LIKE ")
                .push_bind(pattern.clone())
                .push(" OR note LIKE ")
                .push_bind(pattern)
                .push(")");
        }

        push_vacct_clause(&mut qb, account_id, filters.vacct_included.as_deref());

        qb.push(" ORDER BY date, id");

        let rows = qb.build().fetch_all(&self.pool).await?;
        rows.into_iter().map(row_to_transaction).collect()
    }

    async fn recent_by_description(
        &self,
        account_ids: &[String],
        text: &str,
        limit: i64,
    ) -> Result<Vec<Transaction>, sqlx::Error> {
        if account_ids.is_empty() {
            return Ok(Vec::new());
        }
        // `!` as escape char: `ESCAPE '\'` would need different quoting on
        // SQLite and MariaDB.
        let escaped = text
            .replace('!', "!!")
            .replace('%', "!%")
            .replace('_', "!_");
        let mut qb = sqlx::QueryBuilder::<sqlx::Any>::new(format!(
            "SELECT {TX_COLUMNS} FROM transactions WHERE account_id IN ("
        ));
        let mut sep = qb.separated(", ");
        for id in account_ids {
            sep.push_bind(id.clone());
        }
        qb.push(
            ") AND pair_id IS NULL AND description IS NOT NULL AND description <> '' \
                 AND LOWER(description) LIKE ",
        )
        .push_bind(format!("%{}%", escaped.to_lowercase()))
        .push(" ESCAPE '!' ORDER BY date DESC, id DESC LIMIT ")
        .push_bind(limit);

        let rows = qb.build().fetch_all(&self.pool).await?;
        rows.into_iter().map(row_to_transaction).collect()
    }

    async fn sum_cents_before(
        &self,
        account_id: &str,
        date: NaiveDate,
    ) -> Result<i64, sqlx::Error> {
        let result = sqlx::query_scalar(
            "SELECT COALESCE(SUM(cents), 0) FROM transactions WHERE account_id = $1 AND date < $2",
        )
        .bind(account_id)
        .bind(date_iso(date))
        .fetch_one(&self.pool)
        .await?;
        Ok(result)
    }

    async fn sum_cents_before_filtered(
        &self,
        account_id: &str,
        date: NaiveDate,
        vacct_included: Option<&[String]>,
    ) -> Result<i64, sqlx::Error> {
        let mut qb = sqlx::QueryBuilder::<sqlx::Any>::new(
            "SELECT COALESCE(SUM(cents), 0) FROM transactions WHERE account_id = ",
        );
        qb.push_bind(account_id.to_string());
        qb.push(" AND date < ").push_bind(date_iso(date));
        push_vacct_clause(&mut qb, account_id, vacct_included);
        let row = qb.build().fetch_one(&self.pool).await?;
        let result: i64 = row.try_get(0)?;
        Ok(result)
    }

    async fn list_all_rows(&self, account_id: &str) -> Result<Vec<Transaction>, sqlx::Error> {
        let rows = sqlx::query(AssertSqlSafe(format!(
            "SELECT {TX_COLUMNS} FROM transactions WHERE account_id = $1 ORDER BY date, id"
        )))
        .bind(account_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_transaction).collect()
    }

    #[allow(clippy::too_many_arguments)]
    async fn create_transfer_pair(
        &self,
        pair_id: &str,
        from_account_id: &str,
        to_account_id: &str,
        date: NaiveDate,
        cents: i64,
        description: &Option<String>,
        note: &Option<String>,
        created_by: &str,
        vacct_id: Option<&str>,
    ) -> Result<(Transaction, Transaction), sqlx::Error> {
        let now = now_rfc3339();
        let date_s = date_iso(date);

        let mut conn = self.pool.acquire().await?;
        let mut tx = conn.begin().await?;

        let from_tx_id = uuid::Uuid::new_v4().to_string();
        let to_tx_id = uuid::Uuid::new_v4().to_string();

        sqlx::query(
            "INSERT INTO transactions \
             (id, client_id, account_id, date, cents, description, category_id, note, \
              pair_id, rec_id, rule_id, override, computed, vacct_id, import_hash, \
              created_by, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, NULL, $7, $8, NULL, NULL, NULL, NULL, $9, NULL, $10, $11, $12)",
        )
        .bind(&from_tx_id)
        .bind(&from_tx_id)
        .bind(from_account_id)
        .bind(&date_s)
        .bind(-cents)
        .bind(description)
        .bind(note)
        .bind(pair_id)
        .bind(vacct_id)
        .bind(created_by)
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO transactions \
             (id, client_id, account_id, date, cents, description, category_id, note, \
              pair_id, rec_id, rule_id, override, computed, vacct_id, import_hash, \
              created_by, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, NULL, $7, $8, NULL, NULL, NULL, NULL, $9, NULL, $10, $11, $12)",
        )
        .bind(&to_tx_id)
        .bind(&to_tx_id)
        .bind(to_account_id)
        .bind(&date_s)
        .bind(cents)
        .bind(description)
        .bind(note)
        .bind(pair_id)
        .bind(vacct_id)
        .bind(created_by)
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        let from_tx = Transaction {
            id: from_tx_id.clone(),
            client_id: from_tx_id,
            account_id: from_account_id.to_string(),
            date,
            cents: -cents,
            description: description.clone(),
            category_id: None,
            note: note.clone(),
            pair_id: Some(pair_id.to_string()),
            rec_id: None,
            rule_id: None,
            override_: None,
            computed: None,
            vacct_id: vacct_id.map(|s| s.to_string()),
            import_hash: None,
            created_by: created_by.to_string(),
            created_at: now.clone(),
            updated_at: now.clone(),
        };

        let to_tx = Transaction {
            id: to_tx_id.clone(),
            client_id: to_tx_id,
            account_id: to_account_id.to_string(),
            date,
            cents,
            description: description.clone(),
            category_id: None,
            note: note.clone(),
            pair_id: Some(pair_id.to_string()),
            rec_id: None,
            rule_id: None,
            override_: None,
            computed: None,
            vacct_id: vacct_id.map(|s| s.to_string()),
            import_hash: None,
            created_by: created_by.to_string(),
            created_at: now.clone(),
            updated_at: now,
        };

        Ok((from_tx, to_tx))
    }

    async fn delete_transfer_pair(&self, pair_id: &str) -> Result<(), sqlx::Error> {
        let mut conn = self.pool.acquire().await?;
        let mut tx = conn.begin().await?;

        sqlx::query("DELETE FROM transactions WHERE pair_id = $1")
            .bind(pair_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(())
    }

    async fn get_transfer_legs(&self, pair_id: &str) -> Result<Vec<Transaction>, sqlx::Error> {
        let sql =
            format!("SELECT {TX_COLUMNS} FROM transactions WHERE pair_id = $1 ORDER BY account_id");
        let rows = sqlx::query(AssertSqlSafe(sql))
            .bind(pair_id)
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_transaction).collect()
    }

    async fn list_transfer_legs(
        &self,
        from: NaiveDate,
        to: NaiveDate,
        from_account_id: Option<&str>,
        to_account_id: Option<&str>,
    ) -> Result<Vec<Transaction>, sqlx::Error> {
        let mut qb = sqlx::QueryBuilder::<sqlx::Any>::new(format!(
            "SELECT {TX_COLUMNS} FROM transactions WHERE pair_id IS NOT NULL \
             AND date >= "
        ));
        qb.push_bind(date_iso(from));
        qb.push(" AND date <= ").push_bind(date_iso(to));

        if let Some(acc) = from_account_id {
            qb.push(" AND pair_id IN (SELECT pair_id FROM transactions WHERE account_id = ");
            qb.push_bind(acc.to_string());
            qb.push(" AND pair_id IS NOT NULL)");
        }
        if let Some(acc) = to_account_id {
            qb.push(" AND pair_id IN (SELECT pair_id FROM transactions WHERE account_id = ");
            qb.push_bind(acc.to_string());
            qb.push(" AND pair_id IS NOT NULL)");
        }

        qb.push(" ORDER BY date, id");

        let rows = qb.build().fetch_all(&self.pool).await?;
        rows.into_iter().map(row_to_transaction).collect()
    }

    async fn bulk_delete(&self, ids: &[String]) -> Result<u64, sqlx::Error> {
        if ids.is_empty() {
            return Ok(0);
        }

        let mut qb = sqlx::QueryBuilder::<sqlx::Any>::new("DELETE FROM transactions WHERE id IN (");
        qb.push_values(ids, |mut values, id| {
            values.push_bind(id.clone());
        });
        qb.push(")");

        let mut conn = self.pool.acquire().await?;
        let mut tx = conn.begin().await?;

        let result = qb.build().execute(&mut *tx).await?;
        tx.commit().await?;

        Ok(result.rows_affected())
    }

    async fn bulk_recategorize(
        &self,
        ids: &[String],
        category_id: Option<&str>,
    ) -> Result<u64, sqlx::Error> {
        if ids.is_empty() {
            return Ok(0);
        }

        let updated_at = now_rfc3339();

        let mut qb = sqlx::QueryBuilder::<sqlx::Any>::new("UPDATE transactions SET category_id = ");
        qb.push_bind(category_id.map(String::from));
        qb.push(", updated_at = ").push_bind(updated_at);
        qb.push(" WHERE id IN (");
        qb.push_values(ids, |mut values, id| {
            values.push_bind(id.clone());
        });
        qb.push(")");

        let mut conn = self.pool.acquire().await?;
        let mut tx = conn.begin().await?;

        let result = qb.build().execute(&mut *tx).await?;
        tx.commit().await?;

        Ok(result.rows_affected())
    }

    async fn list_for_recurrence(
        &self,
        rec_id: &str,
        from: Option<NaiveDate>,
    ) -> Result<Vec<Transaction>, sqlx::Error> {
        let mut qb = sqlx::QueryBuilder::<sqlx::Any>::new(format!(
            "SELECT {TX_COLUMNS} FROM transactions WHERE rec_id = "
        ));
        qb.push_bind(rec_id.to_string());
        if let Some(from) = from {
            qb.push(" AND date >= ").push_bind(date_iso(from));
        }
        qb.push(" ORDER BY date, id");
        let rows = qb.build().fetch_all(&self.pool).await?;
        rows.into_iter().map(row_to_transaction).collect()
    }

    async fn set_cents_bulk(&self, updates: &[(String, i64)]) -> Result<u64, sqlx::Error> {
        if updates.is_empty() {
            return Ok(0);
        }
        let now = now_rfc3339();
        let mut conn = self.pool.acquire().await?;
        let mut tx = conn.begin().await?;
        let mut n = 0;
        for (id, cents) in updates {
            let res =
                sqlx::query("UPDATE transactions SET cents = $2, updated_at = $3 WHERE id = $1")
                    .bind(id)
                    .bind(*cents)
                    .bind(&now)
                    .execute(&mut *tx)
                    .await?;
            n += res.rows_affected();
        }
        tx.commit().await?;
        Ok(n)
    }

    async fn real_occurrences_for_recurrence(
        &self,
        rec_id: &str,
    ) -> Result<Vec<RealOccurrence>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT date, cents FROM transactions WHERE rec_id = $1 AND override = 1 ORDER BY date",
        )
        .bind(rec_id)
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let date: String = row.try_get("date")?;
            let cents: i64 = row.try_get("cents")?;
            out.push(RealOccurrence {
                date: parse_date(&date)?,
                amount_cents: cents,
            });
        }
        Ok(out)
    }

    async fn sum_noncomputed_cents_before(
        &self,
        account_id: &str,
        date: NaiveDate,
    ) -> Result<i64, sqlx::Error> {
        let result = sqlx::query_scalar(
            "SELECT COALESCE(SUM(cents), 0) FROM transactions \
             WHERE account_id = $1 AND date < $2 AND (computed IS NULL OR computed = 0)",
        )
        .bind(account_id)
        .bind(date_iso(date))
        .fetch_one(&self.pool)
        .await?;
        Ok(result)
    }

    async fn list_noncomputed(
        &self,
        account_id: &str,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<Transaction>, sqlx::Error> {
        let rows = sqlx::query(AssertSqlSafe(format!(
            "SELECT {TX_COLUMNS} FROM transactions \
             WHERE account_id = $1 AND date >= $2 AND date <= $3 \
               AND (computed IS NULL OR computed = 0) ORDER BY date, id"
        )))
        .bind(account_id)
        .bind(date_iso(from))
        .bind(date_iso(to))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_transaction).collect()
    }

    async fn regenerate_rule_rows(
        &self,
        rule: &AccountRule,
        created_by: &str,
        from: NaiveDate,
        to: NaiveDate,
        rows: &[FeeRow],
    ) -> Result<(), sqlx::Error> {
        let now = now_rfc3339();

        let mut conn = self.pool.acquire().await?;
        let mut tx = conn.begin().await?;

        // Delete only this rule's engine rows inside the horizon. Manual rows
        // (computed null/0) and other rules' rows are left untouched.
        sqlx::query(
            "DELETE FROM transactions \
             WHERE rule_id = $1 AND account_id = $2 AND computed = 1 \
               AND date >= $3 AND date <= $4",
        )
        .bind(&rule.id)
        .bind(&rule.account_id)
        .bind(date_iso(from))
        .bind(date_iso(to))
        .execute(&mut *tx)
        .await?;

        for fee in rows {
            let id = uuid::Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO transactions \
                 (id, client_id, account_id, date, cents, description, category_id, note, \
                  pair_id, rec_id, rule_id, override, computed, vacct_id, import_hash, \
                  created_by, created_at, updated_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, NULL, NULL, NULL, $8, NULL, 1, NULL, NULL, $9, $10, $11)",
            )
            .bind(&id)
            .bind(&id)
            .bind(&rule.account_id)
            .bind(date_iso(fee.date))
            .bind(fee.amount_cents)
            .bind(&rule.name)
            .bind(&rule.category_id)
            .bind(&rule.id)
            .bind(created_by)
            .bind(&now)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn delete_rule_rows(&self, rule_id: &str, account_id: &str) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            "DELETE FROM transactions WHERE rule_id = $1 AND account_id = $2 AND computed = 1",
        )
        .bind(rule_id)
        .bind(account_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn list_card_cycle(
        &self,
        account_id: &str,
        after: NaiveDate,
        closing: NaiveDate,
    ) -> Result<Vec<Transaction>, sqlx::Error> {
        let rows = sqlx::query(AssertSqlSafe(format!(
            "SELECT {TX_COLUMNS} FROM transactions \
             WHERE account_id = $1 AND date > $2 AND date <= $3 \
               AND pair_id IS NULL AND (computed IS NULL OR computed = 0) \
             ORDER BY date, id"
        )))
        .bind(account_id)
        .bind(date_iso(after))
        .bind(date_iso(closing))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_transaction).collect()
    }

    async fn sum_all_cents(&self, account_id: &str) -> Result<i64, sqlx::Error> {
        let result = sqlx::query_scalar(
            "SELECT COALESCE(SUM(cents), 0) FROM transactions WHERE account_id = $1",
        )
        .bind(account_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(result)
    }

    async fn sum_raw_after(
        &self,
        account_id: &str,
        after_date: NaiveDate,
        until_date: NaiveDate,
    ) -> Result<i64, sqlx::Error> {
        let result = sqlx::query_scalar(
            "SELECT COALESCE(SUM(cents), 0) FROM transactions \
             WHERE account_id = $1 AND pair_id IS NULL AND date > $2 AND date <= $3",
        )
        .bind(account_id)
        .bind(date_iso(after_date))
        .bind(date_iso(until_date))
        .fetch_one(&self.pool)
        .await?;
        Ok(result)
    }

    async fn min_raw_date(&self, account_id: &str) -> Result<Option<NaiveDate>, sqlx::Error> {
        let result: Option<String> = sqlx::query_scalar(
            "SELECT MIN(date) FROM transactions WHERE account_id = $1 AND pair_id IS NULL",
        )
        .bind(account_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(result.and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok()))
    }

    async fn min_noncomputed_date(
        &self,
        account_id: &str,
    ) -> Result<Option<NaiveDate>, sqlx::Error> {
        let result: Option<String> = sqlx::query_scalar(
            "SELECT MIN(date) FROM transactions \
             WHERE account_id = $1 AND (computed IS NULL OR computed = 0)",
        )
        .bind(account_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(result.and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok()))
    }

    async fn delete_card_charge_pair(&self, pair_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM transactions WHERE pair_id = $1")
            .bind(pair_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn upsert_card_reset_leg(
        &self,
        pair_id: &str,
        card_account_id: &str,
        card_date: NaiveDate,
        card_cents: i64,
        description: &Option<String>,
        created_by: &str,
    ) -> Result<(), sqlx::Error> {
        let now = now_rfc3339();
        let cat_in = fallback_category_id(&self.pool, "in").await?;
        let mut conn = self.pool.acquire().await?;
        let mut tx = conn.begin().await?;

        sqlx::query("DELETE FROM transactions WHERE pair_id = $1 AND account_id <> $2")
            .bind(pair_id)
            .bind(card_account_id)
            .execute(&mut *tx)
            .await?;
        let updated = sqlx::query(
            "UPDATE transactions SET cents = $1, updated_at = $2 \
             WHERE pair_id = $3 AND account_id = $4",
        )
        .bind(card_cents)
        .bind(&now)
        .bind(pair_id)
        .bind(card_account_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if updated == 0 {
            let id = uuid::Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO transactions \
                 (id, client_id, account_id, date, cents, description, category_id, note, \
                  pair_id, rec_id, rule_id, override, computed, vacct_id, import_hash, \
                  created_by, created_at, updated_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $10, NULL, $7, NULL, NULL, NULL, NULL, NULL, NULL, $8, $9, $9)",
            )
            .bind(&id)
            .bind(&id)
            .bind(card_account_id)
            .bind(date_iso(card_date))
            .bind(card_cents)
            .bind(description)
            .bind(pair_id)
            .bind(created_by)
            .bind(&now)
            .bind(&cat_in)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn create_card_charge_pair(
        &self,
        pair_id: &str,
        primary_account_id: &str,
        card_account_id: &str,
        card_date: NaiveDate,
        backing_date: NaiveDate,
        card_cents: i64,
        backing_cents: i64,
        description: &Option<String>,
        created_by: &str,
    ) -> Result<(Transaction, Transaction), sqlx::Error> {
        let now = now_rfc3339();
        let cat_out = fallback_category_id(&self.pool, "out").await?;
        let cat_in = fallback_category_id(&self.pool, "in").await?;
        let card_date_s = date_iso(card_date);
        let backing_date_s = date_iso(backing_date);

        let mut conn = self.pool.acquire().await?;
        let mut tx = conn.begin().await?;

        // A settlement that already exists (both legs present) keeps its dates
        // and only has its amounts refreshed: moving it is up to the user. If a
        // leg is missing (the user deleted the settlement) it is rebuilt from
        // scratch on the current dates.
        let existing: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM transactions WHERE pair_id = $1")
                .bind(pair_id)
                .fetch_one(&mut *tx)
                .await?;
        if existing == 2 {
            for (account_id, cents) in [
                (primary_account_id, -backing_cents),
                (card_account_id, card_cents),
            ] {
                sqlx::query(
                    "UPDATE transactions SET cents = $1, updated_at = $2                      WHERE pair_id = $3 AND account_id = $4",
                )
                .bind(cents)
                .bind(&now)
                .bind(pair_id)
                .bind(account_id)
                .execute(&mut *tx)
                .await?;
            }
            tx.commit().await?;
            drop(conn);
            let legs = self.get_transfer_legs(pair_id).await?;
            let mut primary = None;
            let mut card = None;
            for leg in legs {
                if leg.account_id == primary_account_id {
                    primary = Some(leg);
                } else {
                    card = Some(leg);
                }
            }
            return match (primary, card) {
                (Some(p), Some(c)) => Ok((p, c)),
                _ => Err(sqlx::Error::RowNotFound),
            };
        }
        sqlx::query("DELETE FROM transactions WHERE pair_id = $1")
            .bind(pair_id)
            .execute(&mut *tx)
            .await?;

        let primary_tx_id = uuid::Uuid::new_v4().to_string();
        let card_tx_id = uuid::Uuid::new_v4().to_string();

        // The backing (primary) account is debited on the charge date: the
        // negative leg, carrying spend plus any rule-defined oneri.
        sqlx::query(
            "INSERT INTO transactions \
             (id, client_id, account_id, date, cents, description, category_id, note, \
              pair_id, rec_id, rule_id, override, computed, vacct_id, import_hash, \
              created_by, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $11, NULL, $7, NULL, NULL, NULL, NULL, NULL, NULL, $8, $9, $10)",
        )
        .bind(&primary_tx_id)
        .bind(&primary_tx_id)
        .bind(primary_account_id)
        .bind(&backing_date_s)
        .bind(-backing_cents)
        .bind(description)
        .bind(pair_id)
        .bind(created_by)
        .bind(&now)
        .bind(&now)
        .bind(&cat_out)
        .execute(&mut *tx)
        .await?;

        // The card account is credited on the closing date: the positive leg,
        // resetting the card's exposure using real spend only.
        sqlx::query(
            "INSERT INTO transactions \
             (id, client_id, account_id, date, cents, description, category_id, note, \
              pair_id, rec_id, rule_id, override, computed, vacct_id, import_hash, \
              created_by, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $11, NULL, $7, NULL, NULL, NULL, NULL, NULL, NULL, $8, $9, $10)",
        )
        .bind(&card_tx_id)
        .bind(&card_tx_id)
        .bind(card_account_id)
        .bind(&card_date_s)
        .bind(card_cents)
        .bind(description)
        .bind(pair_id)
        .bind(created_by)
        .bind(&now)
        .bind(&now)
        .bind(&cat_in)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        let primary_leg = Transaction {
            id: primary_tx_id.clone(),
            client_id: primary_tx_id,
            account_id: primary_account_id.to_string(),
            date: backing_date,
            cents: -backing_cents,
            description: description.clone(),
            category_id: cat_out.clone(),
            note: None,
            pair_id: Some(pair_id.to_string()),
            rec_id: None,
            rule_id: None,
            override_: None,
            computed: None,
            vacct_id: None,
            import_hash: None,
            created_by: created_by.to_string(),
            created_at: now.clone(),
            updated_at: now.clone(),
        };

        let card_leg = Transaction {
            id: card_tx_id.clone(),
            client_id: card_tx_id,
            account_id: card_account_id.to_string(),
            date: card_date,
            cents: card_cents,
            description: description.clone(),
            category_id: cat_in.clone(),
            note: None,
            pair_id: Some(pair_id.to_string()),
            rec_id: None,
            rule_id: None,
            override_: None,
            computed: None,
            vacct_id: None,
            import_hash: None,
            created_by: created_by.to_string(),
            created_at: now.clone(),
            updated_at: now,
        };

        Ok((primary_leg, card_leg))
    }

    #[allow(clippy::too_many_arguments)]
    async fn create_deposit_interest_pair(
        &self,
        pair_id: &str,
        deposit_account_id: &str,
        primary_account_id: &str,
        date: NaiveDate,
        gross_cents: i64,
        net_cents: i64,
        description: &Option<String>,
        created_by: &str,
    ) -> Result<(Transaction, Transaction), sqlx::Error> {
        let now = now_rfc3339();
        let cat_in = fallback_category_id(&self.pool, "in").await?;
        let date_s = date_iso(date);

        let mut conn = self.pool.acquire().await?;
        let mut tx = conn.begin().await?;

        // Replace any prior consolidation sharing this deterministic pair id so
        // a re-run updates the interest instead of stacking a second one.
        sqlx::query("DELETE FROM transactions WHERE pair_id = $1")
            .bind(pair_id)
            .execute(&mut *tx)
            .await?;

        let deposit_tx_id = uuid::Uuid::new_v4().to_string();
        let primary_tx_id = uuid::Uuid::new_v4().to_string();

        // The deposit account is credited with the gross interest.
        sqlx::query(
            "INSERT INTO transactions \
             (id, client_id, account_id, date, cents, description, category_id, note, \
              pair_id, rec_id, rule_id, override, computed, vacct_id, import_hash, \
              created_by, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $11, NULL, $7, NULL, NULL, NULL, NULL, NULL, NULL, $8, $9, $10)",
        )
        .bind(&deposit_tx_id)
        .bind(&deposit_tx_id)
        .bind(deposit_account_id)
        .bind(&date_s)
        .bind(gross_cents)
        .bind(description)
        .bind(pair_id)
        .bind(created_by)
        .bind(&now)
        .bind(&now)
        .bind(&cat_in)
        .execute(&mut *tx)
        .await?;

        // The primary (current) account is credited with the net interest.
        sqlx::query(
            "INSERT INTO transactions \
             (id, client_id, account_id, date, cents, description, category_id, note, \
              pair_id, rec_id, rule_id, override, computed, vacct_id, import_hash, \
              created_by, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $11, NULL, $7, NULL, NULL, NULL, NULL, NULL, NULL, $8, $9, $10)",
        )
        .bind(&primary_tx_id)
        .bind(&primary_tx_id)
        .bind(primary_account_id)
        .bind(&date_s)
        .bind(net_cents)
        .bind(description)
        .bind(pair_id)
        .bind(created_by)
        .bind(&now)
        .bind(&now)
        .bind(&cat_in)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        let deposit_leg = Transaction {
            id: deposit_tx_id.clone(),
            client_id: deposit_tx_id,
            account_id: deposit_account_id.to_string(),
            date,
            cents: gross_cents,
            description: description.clone(),
            category_id: cat_in.clone(),
            note: None,
            pair_id: Some(pair_id.to_string()),
            rec_id: None,
            rule_id: None,
            override_: None,
            computed: None,
            vacct_id: None,
            import_hash: None,
            created_by: created_by.to_string(),
            created_at: now.clone(),
            updated_at: now.clone(),
        };

        let primary_leg = Transaction {
            id: primary_tx_id.clone(),
            client_id: primary_tx_id,
            account_id: primary_account_id.to_string(),
            date,
            cents: net_cents,
            description: description.clone(),
            category_id: cat_in.clone(),
            note: None,
            pair_id: Some(pair_id.to_string()),
            rec_id: None,
            rule_id: None,
            override_: None,
            computed: None,
            vacct_id: None,
            import_hash: None,
            created_by: created_by.to_string(),
            created_at: now.clone(),
            updated_at: now,
        };

        Ok((deposit_leg, primary_leg))
    }

    async fn batch_import(
        &self,
        account_id: &str,
        rows: &[ImportRowData],
        created_by: &str,
    ) -> Result<u64, sqlx::Error> {
        if rows.is_empty() {
            return Ok(0);
        }

        let now = now_rfc3339();
        let mut conn = self.pool.acquire().await?;
        let mut tx = conn.begin().await?;

        for row in rows {
            let id = uuid::Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO transactions \
                 (id, client_id, account_id, date, cents, description, category_id, note, \
                  pair_id, rec_id, rule_id, override, computed, vacct_id, import_hash, \
                  created_by, created_at, updated_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, NULL, NULL, \
                  NULL, NULL, NULL, NULL, NULL, NULL, $7, $8, $9, $10)",
            )
            .bind(&id)
            .bind(&id)
            .bind(account_id)
            .bind(date_iso(row.date))
            .bind(row.cents)
            .bind(&row.description)
            .bind(&row.import_hash)
            .bind(created_by)
            .bind(&now)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(rows.len() as u64)
    }
}

/// Map a `transactions` row into a [`Transaction`], decoding the date string
/// and the INTEGER boolean columns.
fn row_to_transaction(row: AnyRow) -> Result<Transaction, sqlx::Error> {
    let date_str: String = row.try_get("date")?;
    let override_val: Option<i64> = row.try_get("override")?;
    let computed_val: Option<i64> = row.try_get("computed")?;

    Ok(Transaction {
        id: row.try_get("id")?,
        client_id: row.try_get("client_id")?,
        account_id: row.try_get("account_id")?,
        date: parse_date(&date_str)?,
        cents: row.try_get("cents")?,
        description: row.try_get("description")?,
        category_id: row.try_get("category_id")?,
        note: row.try_get("note")?,
        pair_id: row.try_get("pair_id")?,
        rec_id: row.try_get("rec_id")?,
        rule_id: row.try_get("rule_id")?,
        override_: override_val.map(|v| v != 0),
        computed: computed_val.map(|v| v != 0),
        vacct_id: row.try_get("vacct_id")?,
        import_hash: row.try_get("import_hash")?,
        created_by: row.try_get("created_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

/// Id of the non-deletable fallback category for `kind` (`"out"` / `"in"`),
/// used to classify engine-generated settlement legs so category filters
/// and statistics include them.
async fn fallback_category_id(
    pool: &sqlx::AnyPool,
    kind: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT id FROM categories WHERE is_fallback = 1 AND kind = $1 LIMIT 1")
        .bind(kind)
        .fetch_optional(pool)
        .await
}
