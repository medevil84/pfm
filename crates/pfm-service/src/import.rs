//! Import use-cases: CSV preview (dry-run) and commit (idempotent write).
//!
//! The [`ImportService`] orchestrates:
//! - CSV parsing (delegated to [`pfm_domain::parse_csv`])
//! - Profile lookup / auto-detection
//! - Deduplication (same date + amount + description on the same account)
//! - Idempotency via `import_hash` (SHA-256 of account+date+cents+description)
//! - Transactional commit of new rows (via [`TransactionsRepo::batch_import`])
//! - Profile persistence after a successful import

use pfm_domain::{
    can_see, can_write, parse_csv, Account, ColumnRole, DateFormat, DecimalSep, DedupKey,
    ImportProfile, ImportRow,
};
use pfm_repo::{
    AccountsRepo, AuditRepo, ImportProfilesRepo, ImportRowData, SharesRepo, SqlxAccountsRepo,
    SqlxImportProfilesRepo, SqlxSharesRepo, SqlxTransactionsRepo, Storage, TransactionsRepo,
    UsersRepo,
};
use thiserror::Error;

use sha2::{Digest, Sha256};

// ─── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("account not found")]
    AccountNotFound,
    #[error("forbidden")]
    Forbidden,
    #[error("profile not found")]
    ProfileNotFound,
    #[error("parse error: {0}")]
    Parse(String),
    #[error("internal error: {0}")]
    Internal(String),
}

// ─── Result types ─────────────────────────────────────────────────────────────

/// A row that will be (or was) written by an import.
#[derive(Debug, Clone)]
pub struct ImportedRow {
    /// The parsed date.
    pub date: String,
    /// Signed amount in cents.
    pub cents: i64,
    /// Description (if any).
    pub description: Option<String>,
    /// `true` if this row matches an existing row in the database (duplicate).
    pub is_duplicate: bool,
    /// `true` if this row's import_hash already exists (previously imported).
    pub already_imported: bool,
    /// `true` if this row will actually be inserted (not a duplicate, not
    /// previously imported).
    pub will_import: bool,
}

/// The result of a preview (dry-run) import.
#[derive(Debug, Clone)]
pub struct PreviewResult {
    /// The bank name used for this import.
    pub bank_name: String,
    /// The profile id (if one was found or created).
    pub profile_id: Option<String>,
    /// Total rows parsed from the CSV.
    pub total_rows: usize,
    /// Rows that match an existing row in the database.
    pub duplicates: Vec<ImportedRow>,
    /// Rows whose import_hash already exists (previously imported).
    pub already_imported: Vec<ImportedRow>,
    /// Rows that will be newly inserted on commit.
    pub to_import: Vec<ImportedRow>,
    /// Rows that could not be parsed (skipped).
    pub skipped: usize,
}

/// The result of a commit import.
#[derive(Debug, Clone)]
pub struct CommitResult {
    /// The profile id used for this import.
    pub profile_id: Option<String>,
    /// Number of rows actually inserted.
    pub inserted: u64,
    /// Number of rows skipped (duplicates or already imported).
    pub skipped: u64,
    /// Total rows in the CSV.
    pub total_rows: usize,
}

// ─── Import parameters ────────────────────────────────────────────────────────

/// Parameters for an import operation (shared by preview and commit).
#[derive(Debug, Clone)]
pub struct ImportParams {
    /// The target account.
    pub account_id: String,
    /// The bank name (used for profile lookup/creation).
    pub bank_name: String,
    /// The raw CSV text.
    pub csv_text: String,
    /// Explicit override for the delimiter. If `None`, the profile's value
    /// (or `;` default) is used.
    pub delimiter: Option<char>,
    /// Explicit override for the date format. If `None`, the profile's value
    /// (or `Dmy` default) is used.
    pub date_format: Option<DateFormat>,
    /// Explicit override for the decimal separator. If `None`, the profile's
    /// value (or `Comma` default) is used.
    pub decimal_sep: Option<DecimalSep>,
    /// Explicit column mapping. If `None`, auto-detection is used.
    pub col_map: Option<Vec<ColumnRole>>,
}

// ─── Row classification ──────────────────────────────────────────────────────

/// The classification of a parsed row relative to the existing database state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowClassification {
    /// The row will be newly inserted.
    New,
    /// The row matches an existing row (same date + amount + description).
    Duplicate,
    /// The row's import_hash already exists (previously imported).
    AlreadyImported,
}

// ─── ImportService ────────────────────────────────────────────────────────────

pub struct ImportService {
    profiles: SqlxImportProfilesRepo,
    transactions: SqlxTransactionsRepo,
    accounts: SqlxAccountsRepo,
    shares: SqlxSharesRepo,
    users: pfm_repo::SqlxAuthRepo,
}

impl ImportService {
    pub fn new(storage: &Storage) -> Self {
        Self {
            profiles: SqlxImportProfilesRepo::new(storage),
            transactions: SqlxTransactionsRepo::new(storage),
            accounts: SqlxAccountsRepo::new(storage),
            shares: SqlxSharesRepo::new(storage),
            users: pfm_repo::SqlxAuthRepo::new(storage),
        }
    }

    /// Compute the SHA-256 import hash for a row.
    /// Input: account_id + ISO date + cents + description (or empty string).
    pub fn compute_import_hash(
        account_id: &str,
        date: &str,
        cents: i64,
        description: &str,
    ) -> String {
        let mut hasher = Sha256::new();
        hasher.update(account_id.as_bytes());
        hasher.update(b"|");
        hasher.update(date.as_bytes());
        hasher.update(b"|");
        hasher.update(cents.to_string().as_bytes());
        hasher.update(b"|");
        hasher.update(description.as_bytes());
        let result = hasher.finalize();
        hex_encode(&result)
    }

    /// Resolve the effective import configuration by:
    /// 1. Looking up a profile by (account_id, bank_name).
    /// 2. If found, use its settings (overridden by explicit params).
    /// 3. If not found, use defaults / auto-detection.
    async fn resolve_config(
        &self,
        params: &ImportParams,
    ) -> Result<
        (
            char,
            DateFormat,
            DecimalSep,
            Option<Vec<ColumnRole>>,
            Option<String>,
            bool,
        ),
        ImportError,
    > {
        let profile = self
            .profiles
            .get_by_bank(&params.account_id, &params.bank_name)
            .await
            .map_err(|e| ImportError::Internal(e.to_string()))?;

        let (delimiter, date_format, decimal_sep, col_map, profile_id, skip_dup) = match &profile {
            Some(p) => (
                params.delimiter.unwrap_or(p.delimiter),
                params.date_format.unwrap_or(p.date_format),
                params.decimal_sep.unwrap_or(p.decimal_sep),
                params.col_map.clone().or_else(|| p.col_map.clone()),
                Some(p.id.clone()),
                p.skip_dup,
            ),
            None => (
                params.delimiter.unwrap_or(';'),
                params.date_format.unwrap_or(DateFormat::Dmy),
                params.decimal_sep.unwrap_or(DecimalSep::Comma),
                params.col_map.clone(),
                None,
                false,
            ),
        };

        Ok((
            delimiter,
            date_format,
            decimal_sep,
            col_map,
            profile_id,
            skip_dup,
        ))
    }

    /// Load an account and enforce the actor's access to it, using the same
    /// owner / admin / share rules as the rest of the application.
    ///
    /// `write = false` grants access to the owner, an admin, or any share;
    /// `write = true` additionally requires a `Write`-level share for
    /// non-owners. Both preview and commit go through this: reading another
    /// user's ledger to classify rows is a disclosure, not a dry run.
    async fn load_account_checked(
        &self,
        actor_id: &str,
        account_id: &str,
        write: bool,
    ) -> Result<Account, ImportError> {
        let account = self
            .accounts
            .get_by_id(account_id)
            .await
            .map_err(|e| ImportError::Internal(e.to_string()))?
            .ok_or(ImportError::AccountNotFound)?;

        if account.owner_id == actor_id {
            return Ok(account);
        }

        let actor = self
            .users
            .get_by_id(actor_id)
            .await
            .map_err(|e| ImportError::Internal(e.to_string()))?
            .ok_or_else(|| ImportError::Internal("actor not found".into()))?;

        let share = self
            .shares
            .get(&account.id, actor_id)
            .await
            .map_err(|e| ImportError::Internal(e.to_string()))?;

        let allowed = if write {
            can_write(
                actor.role,
                actor_id,
                &account.owner_id,
                share.map(|e| e.level),
            )
        } else {
            can_see(actor.role, actor_id, &account.owner_id, share.is_some())
        };

        if allowed {
            Ok(account)
        } else {
            Err(ImportError::Forbidden)
        }
    }

    /// Parse the CSV and classify each row: duplicate, already imported, or
    /// new. Used by both preview and commit.
    async fn parse_and_classify(
        &self,
        actor_id: &str,
        params: &ImportParams,
    ) -> Result<
        (
            Vec<(ImportRow, RowClassification)>,
            (
                char,
                DateFormat,
                DecimalSep,
                Option<Vec<ColumnRole>>,
                Option<String>,
                bool,
            ),
            usize,
        ),
        ImportError,
    > {
        // Authorization first: `list_all_rows` below reads the whole ledger of
        // `params.account_id`, so an unchecked caller could enumerate another
        // user's movements through the duplicate classification.
        self.load_account_checked(actor_id, &params.account_id, false)
            .await?;

        let (delimiter, date_format, decimal_sep, col_map, profile_id, skip_dup) =
            self.resolve_config(params).await?;

        let col_map_ref = col_map.as_deref();
        let rows = parse_csv(
            &params.csv_text,
            delimiter,
            date_format,
            decimal_sep,
            col_map_ref,
        )
        .map_err(ImportError::Parse)?;

        let existing = self
            .transactions
            .list_all_rows(&params.account_id)
            .await
            .map_err(|e| ImportError::Internal(e.to_string()))?;

        let existing_dedup: std::collections::HashSet<DedupKey> = existing
            .iter()
            .map(|t| DedupKey {
                date: t.date,
                cents: t.cents,
                description: t.description.clone(),
            })
            .collect();

        let existing_hashes: std::collections::HashSet<String> = existing
            .iter()
            .filter_map(|t| t.import_hash.as_deref())
            .map(String::from)
            .collect();

        let account_id = &params.account_id;
        let mut classified: Vec<(ImportRow, RowClassification)> = Vec::with_capacity(rows.len());
        let mut skipped = 0usize;

        for row in &rows {
            if row.cents == 0 {
                skipped += 1;
                continue;
            }

            let date_str = row.date.format("%Y-%m-%d").to_string();
            let desc_str = row.description.as_deref().unwrap_or("");
            let hash = Self::compute_import_hash(account_id, &date_str, row.cents, desc_str);

            let dedup_key = DedupKey::from_row(row);
            let is_duplicate = existing_dedup.contains(&dedup_key);
            let already_imported = existing_hashes.contains(&hash);

            let classification = if already_imported {
                RowClassification::AlreadyImported
            } else if is_duplicate {
                RowClassification::Duplicate
            } else {
                RowClassification::New
            };

            classified.push((row.clone(), classification));
        }

        Ok((
            classified,
            (
                delimiter,
                date_format,
                decimal_sep,
                col_map,
                profile_id,
                skip_dup,
            ),
            skipped,
        ))
    }

    /// Dry-run: parse the CSV, classify rows, return statistics.
    /// **Does not write anything to the database.**
    pub async fn preview(
        &self,
        actor_id: &str,
        params: &ImportParams,
    ) -> Result<PreviewResult, ImportError> {
        let (classified, (_d, _df, _ds, _cm, profile_id, skip_dup), skipped) =
            self.parse_and_classify(actor_id, params).await?;

        let mut duplicates = Vec::new();
        let mut already_imported = Vec::new();
        let mut to_import = Vec::new();

        for (row, cls) in &classified {
            let date_str = row.date.format("%Y-%m-%d").to_string();
            let base = ImportedRow {
                date: date_str,
                cents: row.cents,
                description: row.description.clone(),
                is_duplicate: *cls == RowClassification::Duplicate,
                already_imported: *cls == RowClassification::AlreadyImported,
                will_import: *cls == RowClassification::New,
            };

            match cls {
                RowClassification::Duplicate => {
                    if skip_dup {
                        continue;
                    }
                    duplicates.push(base);
                }
                RowClassification::AlreadyImported => {
                    already_imported.push(base);
                }
                RowClassification::New => {
                    to_import.push(base);
                }
            }
        }

        Ok(PreviewResult {
            bank_name: params.bank_name.clone(),
            profile_id,
            total_rows: classified.len() + skipped,
            duplicates,
            already_imported,
            to_import,
            skipped,
        })
    }

    /// Commit: parse the CSV, insert new rows in a single transaction,
    /// save/update the profile. Idempotent via import_hash.
    pub async fn commit(
        &self,
        actor_id: &str,
        params: &ImportParams,
    ) -> Result<CommitResult, ImportError> {
        let (
            classified,
            (delimiter, date_format, decimal_sep, col_map, _profile_id, _skip_dup),
            _skipped,
        ) = self.parse_and_classify(actor_id, params).await?;

        self.load_account_checked(actor_id, &params.account_id, true)
            .await?;

        // Build the rows to insert (only New classification).
        let to_insert: Vec<ImportRowData> = classified
            .iter()
            .filter(|(_, cls)| *cls == RowClassification::New)
            .map(|(row, _)| {
                let date_str = row.date.format("%Y-%m-%d").to_string();
                let desc_str = row.description.as_deref().unwrap_or("");
                let hash =
                    Self::compute_import_hash(&params.account_id, &date_str, row.cents, desc_str);
                ImportRowData {
                    date: row.date,
                    cents: row.cents,
                    description: row.description.clone(),
                    import_hash: hash,
                }
            })
            .collect();

        let inserted = if !to_insert.is_empty() {
            self.transactions
                .batch_import(&params.account_id, &to_insert, actor_id)
                .await
                .map_err(|e| ImportError::Internal(e.to_string()))?
        } else {
            0
        };

        // Save / update the profile for this bank.
        let profile = ImportProfile {
            id: String::new(),
            account_id: params.account_id.clone(),
            bank_name: params.bank_name.clone(),
            delimiter,
            date_format,
            decimal_sep,
            col_map,
            auto_cat: false,
            skip_dup: _skip_dup,
            created_at: String::new(),
        };

        let saved_profile = self
            .profiles
            .save_or_update(&profile)
            .await
            .map_err(|e| ImportError::Internal(e.to_string()))?;

        // Audit: import committed.
        let event = pfm_domain::AuditEvent::new(pfm_domain::AuditAction::ImportCommitted)
            .with_actor(actor_id)
            .with_entity("account", &params.account_id);
        let _ = self.users.log(&event).await;

        Ok(CommitResult {
            profile_id: Some(saved_profile.id),
            inserted,
            skipped: (classified.len() - to_insert.len()) as u64,
            total_rows: classified.len(),
        })
    }

    /// List all import profiles for a given account.
    pub async fn list_profiles(
        &self,
        actor_id: &str,
        account_id: Option<&str>,
    ) -> Result<Vec<ImportProfile>, ImportError> {
        match account_id {
            Some(acc_id) => {
                self.load_account_checked(actor_id, acc_id, false).await?;
                self.profiles
                    .list_by_account(acc_id)
                    .await
                    .map_err(|e| ImportError::Internal(e.to_string()))
            }
            None => {
                // Only profiles of accounts the actor can see: the unfiltered
                // list would expose every user's bank names and column maps.
                let all = self
                    .profiles
                    .list_all()
                    .await
                    .map_err(|e| ImportError::Internal(e.to_string()))?;
                let mut visible = Vec::with_capacity(all.len());
                for profile in all {
                    if self
                        .load_account_checked(actor_id, &profile.account_id, false)
                        .await
                        .is_ok()
                    {
                        visible.push(profile);
                    }
                }
                Ok(visible)
            }
        }
    }
}

/// Encode a byte slice as a lowercase hex string.
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
