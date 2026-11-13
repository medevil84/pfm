-- 0003__accounts_categories.sql
-- Dialect-neutral accounts + categories schema. Runs cleanly on both SQLite and
-- MariaDB.
--
-- Type notes (carried over from 0002):
--   * Primary keys are VARCHAR(64) holding an application-generated UUID.
--   * Movement / business dates (`start_date`) use `YYYY-MM-DD` TEXT; system
--     timestamps (`created_at`) use `YYYY-MM-DDTHH:MM:SSZ` (UTC). The repository
--     converts between these strings and chrono types at the boundary.
--   * Booleans are stored as INTEGER 0/1.
--   * Amounts are always BIGINT in *cents*; the sign encodes the type
--     (negative = out, positive = in) where applicable.
--   * `currency` is an ISO 4217 code (default 'EUR').

CREATE TABLE IF NOT EXISTS accounts (
    id                VARCHAR(64)  NOT NULL PRIMARY KEY,
    owner_id          VARCHAR(64)  NOT NULL,
    name              VARCHAR(128) NOT NULL,
    kind              VARCHAR(16)  NOT NULL,
    icon              VARCHAR(8)   NOT NULL DEFAULT 'bi-wallet2',
    note              VARCHAR(255) NULL,
    iban              VARCHAR(64)  NULL,
    currency          VARCHAR(8)   NOT NULL DEFAULT 'EUR',
    start_date        VARCHAR(16)  NOT NULL,
    start_balance_cents BIGINT     NOT NULL DEFAULT 0,
    limit_cents       BIGINT       NULL,
    charge_day        INTEGER      NULL,
    primary_account_id VARCHAR(64) NULL,
    rate_pct_bps      INTEGER      NULL,
    goal_cents        BIGINT       NULL,
    min_balance_cents BIGINT       NULL,
    active            INTEGER      NOT NULL DEFAULT 1,
    created_at        VARCHAR(64)  NOT NULL,
    FOREIGN KEY (owner_id) REFERENCES users (id) ON DELETE CASCADE,
    FOREIGN KEY (primary_account_id) REFERENCES accounts (id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_accounts_owner ON accounts (owner_id);
CREATE INDEX IF NOT EXISTS idx_accounts_kind ON accounts (kind);

-- Categories. The `key` is a stable, unique slug generated from the name ONLY
-- at creation time; it never changes on rename (so movement references stay
-- intact). `kind` is fixed at creation and cannot be changed afterwards.
CREATE TABLE IF NOT EXISTS categories (
    id           VARCHAR(64)  NOT NULL PRIMARY KEY,
    key          VARCHAR(128) NOT NULL,
    name         VARCHAR(128) NOT NULL,
    emoji        VARCHAR(8)   NOT NULL DEFAULT '✳️',
    kind         VARCHAR(8)   NOT NULL,
    budget_cents BIGINT       NULL,
    is_fallback  INTEGER      NOT NULL DEFAULT 0,
    created_at   VARCHAR(64)  NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_categories_key ON categories (key);

-- Seed: two non-deletable fallback categories (one per kind).
INSERT INTO categories (id, key, name, emoji, kind, budget_cents, is_fallback, created_at)
VALUES ('cat-fallback-out', 'varie', 'Varie', '✳️', 'out', NULL, 1, '1970-01-01T00:00:00Z');

INSERT INTO categories (id, key, name, emoji, kind, budget_cents, is_fallback, created_at)
VALUES ('cat-fallback-in', 'altre-entrate', 'Altre entrate', '✳️', 'in', NULL, 1, '1970-01-01T00:00:00Z');