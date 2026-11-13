//! Connection pool construction and lifecycle.

use std::str::FromStr;

use sqlx::any::{AnyConnectOptions, AnyPoolOptions};
use sqlx::{AnyConnection, AnyPool};

use crate::Engine;

/// Engine-agnostic settings used to open a pool via [`Storage::connect`].
///
/// Fields that do not apply to the selected engine are ignored. The
/// application layer maps its [`pfm_config::DatabaseConfig`] onto this type.
#[derive(Debug, Clone)]
pub struct DbSettings {
    pub engine: Engine,
    pub sqlite_path: Option<String>,
    pub sqlite_apply_pragmas: bool,
    pub sqlite_max_connections: u32,
    pub mariadb_url: Option<String>,
    pub mariadb_max_connections: u32,
    pub mariadb_min_connections: u32,
}

/// A configured sqlx pool together with the engine that backs it.
///
/// `AnyPool` and [`Engine`] are both `Send + Sync`, so `Storage` is as well,
/// which lets the application share it across axum handlers via `Arc`.
#[derive(Debug)]
pub struct Storage {
    pool: AnyPool,
    engine: Engine,
}

impl Storage {
    /// Open a connection pool for the given settings and, for SQLite, apply the
    /// configured pragmas to every new connection.
    pub async fn connect(settings: &DbSettings) -> Result<Self, sqlx::Error> {
        let url = build_url(settings)?;
        let options = AnyConnectOptions::from_str(&url)?;

        let mut pool_options = AnyPoolOptions::new();

        let max = match settings.engine {
            Engine::Sqlite => settings.sqlite_max_connections,
            Engine::MariaDb => settings.mariadb_max_connections,
        };
        if max > 0 {
            pool_options = pool_options.max_connections(max);
        }
        if settings.engine == Engine::MariaDb && settings.mariadb_min_connections > 0 {
            pool_options = pool_options.min_connections(settings.mariadb_min_connections);
        }

        if settings.engine == Engine::Sqlite && settings.sqlite_apply_pragmas {
            pool_options = pool_options.after_connect(|conn, _meta| {
                Box::pin(async move { apply_sqlite_pragmas(conn).await })
            });
        }

        let pool = pool_options.connect_with(options).await?;
        Ok(Self {
            pool,
            engine: settings.engine,
        })
    }

    /// The underlying pool (for one-off raw queries / health checks).
    pub fn pool(&self) -> &AnyPool {
        &self.pool
    }

    /// The engine this storage was opened against.
    pub fn engine(&self) -> Engine {
        self.engine
    }

    /// Verify the pool can reach the database. Used by the `/readyz` probe.
    pub async fn ping(&self) -> Result<(), sqlx::Error> {
        let mut conn = self.pool.acquire().await?;
        sqlx::query("SELECT 1").execute(&mut *conn).await?;
        Ok(())
    }

    /// Run a sqlx [`Migrator`](sqlx::migrate::Migrator) against this pool.
    pub async fn migrate(&self, migrator: &sqlx::migrate::Migrator) -> Result<(), sqlx::Error> {
        migrator.run(&self.pool).await?;
        Ok(())
    }
}

/// Build the connection URL for the selected engine.
fn build_url(settings: &DbSettings) -> Result<String, sqlx::Error> {
    match settings.engine {
        Engine::Sqlite => {
            let path = settings
                .sqlite_path
                .clone()
                .unwrap_or_else(|| "data/pfm.db".to_string());
            if path == ":memory:" {
                Ok("sqlite::memory:".to_string())
            } else if path.len() > 1 && path.as_bytes()[1] == b':' {
                // Windows absolute path (e.g. C:/Users/...): no extra slashes
                Ok(format!("sqlite:{}?mode=rwc", path.replace('\\', "/")))
            } else if path.starts_with('/') {
                // Unix absolute path: triple slash
                Ok(format!("sqlite://{path}?mode=rwc"))
            } else {
                // relative path
                Ok(format!("sqlite:{path}?mode=rwc"))
            }
        }
        Engine::MariaDb => settings.mariadb_url.clone().ok_or_else(|| {
            sqlx::Error::Configuration(
                "database.mariadb_url is required when engine = \"mariadb\"".into(),
            )
        }),
    }
}

/// Apply the SQLite performance / correctness pragmas to a freshly opened
/// connection. `journal_mode=WAL` is persisted in the database file; the rest
/// are per-connection and therefore (re)applied on every connect.
async fn apply_sqlite_pragmas(conn: &mut AnyConnection) -> Result<(), sqlx::Error> {
    for pragma in [
        "PRAGMA journal_mode=WAL;",
        "PRAGMA synchronous=NORMAL;",
        "PRAGMA busy_timeout=5000;",
        "PRAGMA foreign_keys=ON;",
    ] {
        sqlx::query(pragma).execute(&mut *conn).await?;
    }
    Ok(())
}
