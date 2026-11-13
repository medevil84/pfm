//! Setup-state repository: tracks whether the initial configuration has been
//! completed and provides the atomic "reset all application data" operation.
//!
//! The `setup_state` table (migration 0010) holds a single key-value row:
//! `('initialized', '1')` means the wizard has run and an admin user exists.

use async_trait::async_trait;
use sqlx::{Acquire, AnyPool, AssertSqlSafe, Row};

use crate::dialect::Dialect;
use crate::Storage;

/// Parameters for completing the initial setup (passed to the repo's
/// transactional method).
#[derive(Debug, Clone)]
pub struct SetupCompleteData {
    pub user_id: String,
    pub username: String,
    pub email: String,
    pub password_hash: String,
    pub locale: String,
    pub default_currency: String,
}

/// Read/write access to the setup-state marker and the data-reset operation.
#[async_trait]
pub trait SetupRepo: Send + Sync {
    /// Returns `true` when `setup_state` contains `('initialized', '1')`.
    async fn is_initialized(&self) -> Result<bool, sqlx::Error>;

    /// Returns `true` when at least one user with `role = 'admin'` exists.
    async fn has_admin_user(&self) -> Result<bool, sqlx::Error>;

    /// Delete all application data in FK-safe order, leaving the schema and
    /// migrations intact. The database ends in the same state as a freshly
    /// migrated installation.
    ///
    /// This operation is atomic: either all data is deleted or none is.
    async fn reset_all_data(&self) -> Result<(), sqlx::Error>;

    /// Atomically create the admin user, their preferences row, and the
    /// `setup_state` marker in a single transaction.
    ///
    /// If any step fails, the entire operation is rolled back.
    async fn complete_setup(&self, data: &SetupCompleteData) -> Result<(), sqlx::Error>;
}

/// SQLx-`Any` implementation of [`SetupRepo`].
#[derive(Clone)]
pub struct SqlxSetupRepo {
    pool: AnyPool,
    #[allow(dead_code)]
    dialect: Dialect,
}

impl SqlxSetupRepo {
    /// Build a repository handle from an existing [`Storage`] (shares its pool).
    pub fn new(storage: &Storage) -> Self {
        Self {
            pool: storage.pool().clone(),
            dialect: storage.engine().into(),
        }
    }
}

/// RFC-3339 UTC timestamp for the `updated_at` column.
fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[async_trait]
impl SetupRepo for SqlxSetupRepo {
    async fn is_initialized(&self) -> Result<bool, sqlx::Error> {
        let row = sqlx::query("SELECT value FROM setup_state WHERE key = 'initialized'")
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(r) => {
                let value: String = r.get(0);
                Ok(value == "1")
            }
            None => Ok(false),
        }
    }

    async fn has_admin_user(&self) -> Result<bool, sqlx::Error> {
        let row = sqlx::query("SELECT COUNT(*) FROM users WHERE role = 'admin' AND active = 1")
            .fetch_one(&self.pool)
            .await?;
        let count: i64 = row.get(0);
        Ok(count > 0)
    }

    async fn reset_all_data(&self) -> Result<(), sqlx::Error> {
        // Driven through sqlx's transaction handle rather than raw BEGIN /
        // COMMIT / ROLLBACK statements: with raw statements a failing COMMIT
        // returns the connection to the pool with the transaction still open,
        // and the next borrower inherits it. `Transaction` also rolls back on
        // drop, so an early return can no longer leak an open transaction.
        let mut conn = self.pool.acquire().await?;
        let mut tx = conn.begin().await?;

        let result: Result<(), sqlx::Error> = async {
            // Deletion order respects FK dependencies (children first).
            let tables = [
                "recurrence_overrides",
                "recurrences",
                "account_rules",
                "import_profiles",
                "transactions",
                "account_shares",
                "accounts",
                "user_prefs",
                "audit_log",
                "users",
            ];

            for table in &tables {
                let sql = format!("DELETE FROM {table}");
                sqlx::query(AssertSqlSafe(sql.clone()))
                    .execute(&mut *tx)
                    .await?;
            }

            // Delete non-fallback categories (preserve the 2 seeded fallback rows).
            sqlx::query("DELETE FROM categories WHERE is_fallback = 0")
                .execute(&mut *tx)
                .await?;

            // Clear the setup-state marker.
            sqlx::query("DELETE FROM setup_state WHERE key = 'initialized'")
                .execute(&mut *tx)
                .await?;

            Ok(())
        }
        .await;

        match result {
            Ok(()) => tx.commit().await?,
            Err(e) => {
                tx.rollback().await?;
                return Err(e);
            }
        }
        Ok(())
    }

    async fn complete_setup(&self, data: &SetupCompleteData) -> Result<(), sqlx::Error> {
        let mut conn = self.pool.acquire().await?;
        let mut tx = conn.begin().await?;

        let result: Result<(), sqlx::Error> = async {
            // Create the admin user.
            let now = now_rfc3339();
            sqlx::query(
                "INSERT INTO users \
                 (id, username, email, password_hash, role, active, locale, default_currency, created_at) \
                 VALUES ($1, $2, $3, $4, 'admin', 1, $5, $6, $7)",
            )
            .bind(&data.user_id)
            .bind(&data.username)
            .bind(&data.email)
            .bind(&data.password_hash)
            .bind(&data.locale)
            .bind(&data.default_currency)
            .bind(&now)
            .execute(&mut *tx)
            .await?;

            // Create the user's preferences row.
            sqlx::query(
                "INSERT INTO user_prefs \
                 (user_id, theme, show_sign, privacy, show_m_saldo, empty_days, show_future, rows_above, rows_below, rows_step) \
                 VALUES ($1, 'system', 1, 0, 0, 0, 1, 2, 8, 1)",
            )
            .bind(&data.user_id)
            .execute(&mut *tx)
            .await?;

            // Mark the setup as initialized.
            let setup_now = now_rfc3339();
            sqlx::query(
                "INSERT INTO setup_state (key, value, updated_at) VALUES ('initialized', '1', $1)",
            )
            .bind(&setup_now)
            .execute(&mut *tx)
            .await?;

            Ok(())
        }
        .await;

        match result {
            Ok(()) => tx.commit().await?,
            Err(e) => {
                tx.rollback().await?;
                return Err(e);
            }
        }
        Ok(())
    }
}
