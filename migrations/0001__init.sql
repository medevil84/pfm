-- 0001__init.sql
-- Dialect-neutral bootstrap migration. Runs cleanly on both SQLite and MariaDB.
--
-- Notes:
--   * VARCHAR (not TEXT) for the primary key: MariaDB rejects TEXT/BLOB as a
--     PRIMARY KEY, VARCHAR is accepted by both engines.
--   * TIMESTAMP is accepted as a column type by both engines and supports
--     DEFAULT CURRENT_TIMESTAMP.

CREATE TABLE IF NOT EXISTS schema_meta (
    key        VARCHAR(64)  NOT NULL PRIMARY KEY,
    value      VARCHAR(255) NOT NULL,
    updated_at TIMESTAMP    NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- Plain INSERT is safe: sqlx records applied migrations in _sqlx_migrations
-- and never re-runs a migration that has already been applied.
INSERT INTO schema_meta (key, value) VALUES ('bootstrap', '0001');