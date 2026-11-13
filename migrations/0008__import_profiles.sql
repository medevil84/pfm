-- 0008__import_profiles.sql
-- Dialect-neutral import profiles schema. Runs cleanly on both SQLite and
-- MariaDB.
--
-- `import_profiles` stores per-bank CSV parsing configuration so that a
-- user who imports a statement from the same bank twice does not need to
-- re-configure the column mapping.
--
-- `col_map` is stored as a JSON string (e.g. `[{"col":0,"role":"date"},...]`).
-- An empty or absent col_map means the columns should be auto-detected.

CREATE TABLE IF NOT EXISTS import_profiles (
    id            VARCHAR(64)  NOT NULL PRIMARY KEY,
    account_id    VARCHAR(64)  NOT NULL,
    bank_name     VARCHAR(128) NOT NULL,
    delimiter     VARCHAR(4)   NOT NULL DEFAULT ';',
    date_format   VARCHAR(8)   NOT NULL DEFAULT 'dmy',
    decimal_sep   VARCHAR(4)   NOT NULL DEFAULT ',',
    col_map       TEXT         NULL,
    auto_cat      INTEGER      NOT NULL DEFAULT 0,
    skip_dup      INTEGER      NOT NULL DEFAULT 0,
    created_at    VARCHAR(64)  NOT NULL,
    FOREIGN KEY (account_id) REFERENCES accounts (id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_import_profiles_account
    ON import_profiles (account_id);

CREATE INDEX IF NOT EXISTS idx_import_profiles_bank
    ON import_profiles (account_id, bank_name);