-- 0004__account_shares.sql
-- Account sharing: grants read or write access to a user on an account owned
-- by another user. The owner never appears in this table (full access by
-- construction). Admins bypass all share checks.
--
-- Type notes (consistent with 0001–0003):
--   * Primary key is VARCHAR(64) holding an application-generated UUID.
--   * `level` is 'read' or 'write'.
--   * `created_at` is an RFC-3339 UTC string.

CREATE TABLE IF NOT EXISTS account_shares (
    id         VARCHAR(64) NOT NULL PRIMARY KEY,
    account_id VARCHAR(64) NOT NULL,
    user_id    VARCHAR(64) NOT NULL,
    level      VARCHAR(8)  NOT NULL DEFAULT 'read',
    created_at VARCHAR(64) NOT NULL,
    FOREIGN KEY (account_id) REFERENCES accounts (id) ON DELETE CASCADE,
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_account_shares_unique ON account_shares (account_id, user_id);
CREATE INDEX IF NOT EXISTS idx_account_shares_user ON account_shares (user_id);