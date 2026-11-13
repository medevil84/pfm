//! Dialect-specific SQL fragments.
//!
//! A handful of SQL expressions differ between SQLite and MariaDB. Centralizing
//! them here keeps repository code from hard-coding engine-specific SQL. Only
//! the fragments actually needed are provided in this skeleton step; more
//! (e.g. upsert / `ON CONFLICT` vs `ON DUPLICATE KEY`) arrive with the
//! repositories in later steps.

/// SQL dialect of the active storage engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dialect {
    Sqlite,
    MariaDb,
}

impl Dialect {
    /// Expression returning the id of the most recently inserted row.
    pub fn last_insert_id(self) -> &'static str {
        match self {
            Dialect::Sqlite => "last_insert_rowid()",
            Dialect::MariaDb => "last_insert_id()",
        }
    }

    /// Whether `ON CONFLICT` (SQLite) vs `ON DUPLICATE KEY UPDATE` (MariaDB)
    /// should be used for upserts. Exposed as a simple boolean so repository
    /// code can branch without re-deriving the dialect.
    pub fn uses_on_conflict(self) -> bool {
        matches!(self, Dialect::Sqlite)
    }

    /// Build the engine-specific "on duplicate" suffix for an upsert keyed on a
    /// single conflict column.
    ///
    /// Given an `update_cols` list, this produces:
    ///   * SQLite:   `ON CONFLICT (<conflict_col>) DO UPDATE SET a=excluded.a, ...`
    ///   * MariaDB:  `ON DUPLICATE KEY UPDATE a=VALUES(a), ...`
    ///
    /// The caller prepends the `INSERT INTO ... VALUES (...)` statement.
    pub fn upsert_suffix(self, conflict_col: &str, update_cols: &[&str]) -> String {
        let updates = update_cols
            .iter()
            .map(|c| match self {
                Dialect::Sqlite => format!("{c}=excluded.{c}"),
                Dialect::MariaDb => format!("{c}=VALUES({c})"),
            })
            .collect::<Vec<_>>()
            .join(", ");

        match self {
            Dialect::Sqlite => {
                format!("ON CONFLICT ({conflict_col}) DO UPDATE SET {updates}")
            }
            Dialect::MariaDb => {
                format!("ON DUPLICATE KEY UPDATE {updates}")
            }
        }
    }
}

impl From<crate::Engine> for Dialect {
    fn from(engine: crate::Engine) -> Self {
        match engine {
            crate::Engine::Sqlite => Dialect::Sqlite,
            crate::Engine::MariaDb => Dialect::MariaDb,
        }
    }
}
