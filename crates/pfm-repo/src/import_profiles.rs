//! Import profiles repository: read/write access to the `import_profiles` table.
//!
//! Profiles store per-bank CSV parsing configuration so that repeated imports
//! from the same bank do not require re-configuring the column mapping.

use async_trait::async_trait;
use pfm_domain::{ColumnRole, DateFormat, DecimalSep, ImportProfile};
use sqlx::any::AnyRow;
use sqlx::{AnyPool, Row};

/// Read/write access to import profiles.
#[async_trait]
pub trait ImportProfilesRepo: Send + Sync {
    /// Insert a new profile or update the existing one for the same
    /// `(account_id, bank_name)` pair. Returns the persisted profile.
    async fn save_or_update(&self, profile: &ImportProfile) -> Result<ImportProfile, sqlx::Error>;

    /// Look up a profile by its id.
    async fn get_by_id(&self, id: &str) -> Result<Option<ImportProfile>, sqlx::Error>;

    /// Look up a profile by `(account_id, bank_name)`.
    async fn get_by_bank(
        &self,
        account_id: &str,
        bank_name: &str,
    ) -> Result<Option<ImportProfile>, sqlx::Error>;

    /// List all profiles for a given account.
    async fn list_by_account(&self, account_id: &str) -> Result<Vec<ImportProfile>, sqlx::Error>;

    /// List all profiles (across all accounts).
    async fn list_all(&self) -> Result<Vec<ImportProfile>, sqlx::Error>;
}

/// A single SQLx-`Any` implementation of the import profiles repository.
#[derive(Clone)]
pub struct SqlxImportProfilesRepo {
    pool: AnyPool,
}

impl SqlxImportProfilesRepo {
    pub fn new(storage: &crate::Storage) -> Self {
        Self {
            pool: storage.pool().clone(),
        }
    }
}

/// Serialize a `Vec<ColumnRole>` to a JSON string for storage.
/// Format: `[{"col":0,"role":"date"},...]`
fn col_map_to_json(map: &Option<Vec<ColumnRole>>) -> Option<String> {
    let map = map.as_ref()?;
    if map.is_empty() {
        return None;
    }
    let items: Vec<String> = map
        .iter()
        .enumerate()
        .map(|(i, role)| format!("{{\"col\":{i},\"role\":\"{}\"}}", role.as_str()))
        .collect();
    Some(format!("[{}]", items.join(",")))
}

/// Deserialize a JSON string from the DB into `Vec<ColumnRole>`.
fn col_map_from_json(s: &str) -> Option<Vec<ColumnRole>> {
    let v: Vec<serde_json::Value> = serde_json::from_str(s).ok()?;
    let mut roles = Vec::new();
    for item in v {
        let col = item.get("col")?.as_u64()? as usize;
        let role_str = item.get("role")?.as_str()?;
        let role = ColumnRole::try_from(role_str)?;
        // Pad the vec to the correct index.
        while roles.len() <= col {
            roles.push(ColumnRole::Other);
        }
        roles[col] = role;
    }
    Some(roles)
}

fn row_to_profile(row: AnyRow) -> Result<ImportProfile, sqlx::Error> {
    let date_format_str: String = row.try_get("date_format")?;
    let date_format = DateFormat::try_from(&date_format_str).unwrap_or(DateFormat::Dmy);

    let decimal_sep_str: String = row.try_get("decimal_sep")?;
    let decimal_sep = DecimalSep::try_from(&decimal_sep_str).unwrap_or(DecimalSep::Comma);

    let delimiter_str: String = row.try_get("delimiter")?;
    let delimiter = delimiter_str.chars().next().unwrap_or(';');

    let col_map_json: Option<String> = row.try_get("col_map")?;
    let col_map = col_map_json
        .as_deref()
        .filter(|s| !s.is_empty())
        .and_then(col_map_from_json);

    let auto_cat: Option<i64> = row.try_get("auto_cat")?;
    let skip_dup: Option<i64> = row.try_get("skip_dup")?;

    Ok(ImportProfile {
        id: row.try_get("id")?,
        account_id: row.try_get("account_id")?,
        bank_name: row.try_get("bank_name")?,
        delimiter,
        date_format,
        decimal_sep,
        col_map,
        auto_cat: auto_cat.unwrap_or(0) != 0,
        skip_dup: skip_dup.unwrap_or(0) != 0,
        created_at: row.try_get("created_at")?,
    })
}

#[async_trait]
impl ImportProfilesRepo for SqlxImportProfilesRepo {
    async fn save_or_update(&self, profile: &ImportProfile) -> Result<ImportProfile, sqlx::Error> {
        let col_map_json = col_map_to_json(&profile.col_map);

        // Try to find an existing profile for (account_id, bank_name).
        let existing =
            sqlx::query("SELECT id FROM import_profiles WHERE account_id = $1 AND bank_name = $2")
                .bind(&profile.account_id)
                .bind(&profile.bank_name)
                .fetch_optional(&self.pool)
                .await?;

        if let Some(row) = existing {
            let existing_id: String = row.try_get("id")?;
            sqlx::query(
                "UPDATE import_profiles SET \
                 delimiter = $2, date_format = $3, decimal_sep = $4, \
                 col_map = $5, auto_cat = $6, skip_dup = $7 \
                 WHERE id = $1",
            )
            .bind(&existing_id)
            .bind(profile.delimiter.to_string())
            .bind(profile.date_format.as_str())
            .bind(profile.decimal_sep.as_str())
            .bind(&col_map_json)
            .bind(if profile.auto_cat { 1i64 } else { 0 })
            .bind(if profile.skip_dup { 1i64 } else { 0 })
            .execute(&self.pool)
            .await?;

            Ok(ImportProfile {
                id: existing_id,
                ..profile.clone()
            })
        } else {
            // Insert new profile.
            let id = uuid::Uuid::new_v4().to_string();
            let now = chrono::Utc::now().to_rfc3339();
            sqlx::query(
                "INSERT INTO import_profiles \
                 (id, account_id, bank_name, delimiter, date_format, decimal_sep, \
                  col_map, auto_cat, skip_dup, created_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
            )
            .bind(&id)
            .bind(&profile.account_id)
            .bind(&profile.bank_name)
            .bind(profile.delimiter.to_string())
            .bind(profile.date_format.as_str())
            .bind(profile.decimal_sep.as_str())
            .bind(&col_map_json)
            .bind(if profile.auto_cat { 1i64 } else { 0 })
            .bind(if profile.skip_dup { 1i64 } else { 0 })
            .bind(&now)
            .execute(&self.pool)
            .await?;

            Ok(ImportProfile {
                id,
                created_at: now,
                ..profile.clone()
            })
        }
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<ImportProfile>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT id, account_id, bank_name, delimiter, date_format, decimal_sep, \
             col_map, auto_cat, skip_dup, created_at \
             FROM import_profiles WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_profile).transpose()
    }

    async fn get_by_bank(
        &self,
        account_id: &str,
        bank_name: &str,
    ) -> Result<Option<ImportProfile>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT id, account_id, bank_name, delimiter, date_format, decimal_sep, \
             col_map, auto_cat, skip_dup, created_at \
             FROM import_profiles WHERE account_id = $1 AND bank_name = $2",
        )
        .bind(account_id)
        .bind(bank_name)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_profile).transpose()
    }

    async fn list_by_account(&self, account_id: &str) -> Result<Vec<ImportProfile>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT id, account_id, bank_name, delimiter, date_format, decimal_sep, \
             col_map, auto_cat, skip_dup, created_at \
             FROM import_profiles WHERE account_id = $1 ORDER BY bank_name",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_profile).collect()
    }

    async fn list_all(&self) -> Result<Vec<ImportProfile>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT id, account_id, bank_name, delimiter, date_format, decimal_sep, \
             col_map, auto_cat, skip_dup, created_at \
             FROM import_profiles ORDER BY bank_name",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_profile).collect()
    }
}
