-- 0002__auth.sql
-- Dialect-neutral authentication / preferences / audit schema. Runs cleanly on
-- both SQLite and MariaDB.
--
-- Type notes:
--   * Primary keys are VARCHAR(64) holding an application-generated UUID. This
--     keeps the DDL identical across engines and avoids per-engine id handling
--     (SQLite INTEGER-PK vs MariaDB AUTO_INCREMENT) and last-insert-id shims.
--   * The sqlx `Any` driver (used to share one query set across SQLite and
--     MariaDB) does NOT decode chrono date/time values. To stay dialect- and
--     driver-neutral, every date/time column is VARCHAR storing ISO-8601 text:
--       - movement dates (future) use `YYYY-MM-DD`
--       - system timestamps here use `YYYY-MM-DDTHH:MM:SSZ` (UTC)
--     The repository layer converts these strings to/from chrono types
--     (`NaiveDate`, `DateTime<Utc>`) at the boundary.
--   * Booleans are stored as INTEGER 0/1 (SQLite has no BOOLEAN type; MariaDB
--     stores BOOLEAN as TINYINT(1)). The repository decodes them to Rust bool.
--   * Nullable columns are used for `last_seen_at` (system metadata). VARCHAR
--     accepts NULL on both engines.

CREATE TABLE IF NOT EXISTS users (
    id               VARCHAR(64)  NOT NULL PRIMARY KEY,
    username         VARCHAR(64)  NOT NULL,
    email            VARCHAR(255) NOT NULL,
    password_hash    VARCHAR(255) NOT NULL,
    role             VARCHAR(16)  NOT NULL DEFAULT 'user',
    active           INTEGER      NOT NULL DEFAULT 1,
    locale           VARCHAR(8)   NOT NULL DEFAULT 'it',
    default_currency VARCHAR(8)   NOT NULL DEFAULT 'EUR',
    last_seen_at     VARCHAR(64)  NULL,
    created_at       VARCHAR(64)  NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_users_username ON users (username);
CREATE UNIQUE INDEX IF NOT EXISTS idx_users_email ON users (email);

-- 1:1 display preferences, keyed by user.
CREATE TABLE IF NOT EXISTS user_prefs (
    user_id      VARCHAR(64) NOT NULL PRIMARY KEY,
    theme        VARCHAR(16) NOT NULL DEFAULT 'system',
    show_sign    INTEGER     NOT NULL DEFAULT 1,
    privacy      INTEGER     NOT NULL DEFAULT 0,
    show_m_saldo INTEGER     NOT NULL DEFAULT 0,
    empty_days   INTEGER     NOT NULL DEFAULT 0,
    show_future  INTEGER     NOT NULL DEFAULT 1,
    rows_above   INTEGER     NOT NULL DEFAULT 2,
    rows_below   INTEGER     NOT NULL DEFAULT 8,
    rows_step    INTEGER     NOT NULL DEFAULT 1,
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE
);

-- Append-only audit trail. `actor_user_id` is NULL for failed logins (no valid
-- session yet); `impersonated_as_user_id` is reserved (kept NULL until the
-- impersonation feature lands). `ip`/`user_agent` are nullable VARCHAR.
CREATE TABLE IF NOT EXISTS audit_log (
    id                      VARCHAR(64)  NOT NULL PRIMARY KEY,
    actor_user_id           VARCHAR(64)  NULL,
    impersonated_as_user_id VARCHAR(64)  NULL,
    action                  VARCHAR(32)  NOT NULL,
    entity                  VARCHAR(64)  NULL,
    entity_id               VARCHAR(64)  NULL,
    ip                      VARCHAR(64)  NULL,
    user_agent              VARCHAR(512) NULL,
    at                      VARCHAR(64)  NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_audit_log_actor_at ON audit_log (actor_user_id, at);
CREATE INDEX IF NOT EXISTS idx_audit_log_at ON audit_log (at);