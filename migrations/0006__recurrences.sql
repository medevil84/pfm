-- Recurrence rules and per-date overrides.

CREATE TABLE IF NOT EXISTS recurrences (
    id            VARCHAR(64) PRIMARY KEY,
    owner_id      VARCHAR(64) NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    description   TEXT NOT NULL,
    category_id   VARCHAR(64) NOT NULL REFERENCES categories(id) ON DELETE CASCADE,
    kind          TEXT NOT NULL CHECK (kind IN ('out','in')),
    amount_cents  BIGINT NOT NULL,
    account_id    VARCHAR(64) NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    pattern       TEXT NOT NULL,
    weekend       TEXT NOT NULL,
    first_date    TEXT NOT NULL,
    end_type      TEXT NOT NULL,
    end_date      TEXT,
    end_count     INTEGER,
    mode          TEXT NOT NULL CHECK (mode IN ('fixed','avg')),
    avg_n         INTEGER NOT NULL DEFAULT 0,
    active        INTEGER NOT NULL DEFAULT 1,
    note          TEXT NOT NULL DEFAULT '',
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_recurrences_owner ON recurrences(owner_id);
CREATE INDEX IF NOT EXISTS idx_recurrences_account ON recurrences(account_id);

CREATE TABLE IF NOT EXISTS recurrence_overrides (
    id            VARCHAR(64) PRIMARY KEY,
    rec_id        VARCHAR(64) NOT NULL REFERENCES recurrences(id) ON DELETE CASCADE,
    date          TEXT NOT NULL,
    amount_cents  BIGINT NOT NULL,
    description   TEXT,
    created_at    TEXT NOT NULL,
    UNIQUE (rec_id, date)
);