-- 0010__setup_state.sql
-- Dialect-neutral setup-state tracking table.
--
-- A row with key='initialized' and value='1' marks the application as
-- having completed its first-time configuration (admin user created,
-- preferences set). The absence of this row (or value != '1') means the
-- database is still in "just-migrated" state and the setup wizard is
-- required.
--
-- VARCHAR types are used for full cross-engine compatibility (SQLite + MariaDB).

CREATE TABLE IF NOT EXISTS setup_state (
    key        VARCHAR(32) NOT NULL PRIMARY KEY,
    value      VARCHAR(64) NOT NULL,
    updated_at VARCHAR(64) NOT NULL
);