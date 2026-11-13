-- 0004__transactions.sql
-- Dialect-neutral transactions schema. Runs cleanly on both SQLite and
-- MariaDB.
--
-- Type notes (carried over from 0003):
--   * Primary keys are VARCHAR(64) holding an application-generated UUID.
--   * Movement dates use `YYYY-MM-DD` TEXT; system timestamps use
--     `YYYY-MM-DDTHH:MM:SSZ` (UTC). The repository converts between these
--     strings and chrono types at the boundary.
--   * Amounts are always BIGINT in *cents*; the sign encodes the type
--     (negative = out, positive = in).
--   * `pair_id`, `rec_id`, `rule_id`, `vacct_id`, `override`, `computed`,
--     and `import_hash` are reserved for future features. They are created now
--     so that the schema is stable, but are always NULL in this step.

CREATE TABLE IF NOT EXISTS transactions (
    id            VARCHAR(64)  NOT NULL PRIMARY KEY,
    client_id     VARCHAR(64)  NOT NULL,
    account_id    VARCHAR(64)  NOT NULL,
    date          VARCHAR(16)  NOT NULL,
    cents         BIGINT       NOT NULL,
    description   VARCHAR(255) NULL,
    category_id   VARCHAR(64)  NULL,
    note          VARCHAR(255) NULL,
    pair_id       VARCHAR(64)  NULL,
    rec_id        VARCHAR(64)  NULL,
    rule_id       VARCHAR(64)  NULL,
    override      INTEGER      NULL,
    computed      INTEGER      NULL,
    vacct_id      VARCHAR(64)  NULL,
    import_hash   VARCHAR(64)  NULL,
    created_by    VARCHAR(64)  NOT NULL,
    created_at    VARCHAR(64)  NOT NULL,
    updated_at    VARCHAR(64)  NOT NULL,
    FOREIGN KEY (account_id) REFERENCES accounts (id) ON DELETE CASCADE,
    FOREIGN KEY (category_id) REFERENCES categories (id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_transactions_account_date ON transactions (account_id, date, id);
CREATE INDEX IF NOT EXISTS idx_transactions_pair ON transactions (pair_id);
CREATE INDEX IF NOT EXISTS idx_transactions_rec ON transactions (rec_id);
CREATE INDEX IF NOT EXISTS idx_transactions_category ON transactions (category_id);
CREATE INDEX IF NOT EXISTS idx_transactions_date ON transactions (date);