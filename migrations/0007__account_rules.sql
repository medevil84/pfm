-- 0007__account_rules.sql
-- Dialect-neutral "spending rule" schema (canoni, imposte di bollo, commissioni).
-- Runs cleanly on both SQLite and MariaDB.
--
-- Type notes (carried over from 0003/0005/0006):
--   * Primary keys are VARCHAR(64) holding an application-generated UUID.
--   * String enums (calc_type, cadence, cond_base, cond_op) are stored as TEXT
--     and validated with CHECK constraints.
--   * `value` is BIGINT in cents (fixed) or basis points (percentage types).
--   * `annual_cap_cents`, `charge_month`, `cond_op` and `cond_threshold` are
--     nullable.
--   * Rows the engine materializes live in `transactions` with `rule_id` set
--     and `computed = 1`; this table stores only the rule itself.

CREATE TABLE IF NOT EXISTS account_rules (
    id               VARCHAR(64) PRIMARY KEY,
    account_id       VARCHAR(64) NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name             TEXT NOT NULL,
    category_id      VARCHAR(64) NOT NULL REFERENCES categories(id) ON DELETE CASCADE,
    calc_type        TEXT NOT NULL CHECK (calc_type IN ('fixed','pct_avg_balance','pct_end_balance')),
    value            BIGINT NOT NULL,
    is_annual        INTEGER NOT NULL DEFAULT 0,
    annual_cap_cents BIGINT NULL,
    cadence          TEXT NOT NULL CHECK (cadence IN ('monthly','bimonthly','quarterly','semiannual','annual')),
    charge_day       INTEGER NOT NULL,
    charge_month     INTEGER NULL,
    cond_base        TEXT NOT NULL CHECK (cond_base IN ('always','avg_balance','end_balance','total_out','total_in','tx_count')),
    cond_op          TEXT NULL CHECK (cond_op IS NULL OR cond_op IN ('>','>=','<','<=')),
    cond_threshold   BIGINT NULL,
    active           INTEGER NOT NULL DEFAULT 1,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_account_rules_account ON account_rules(account_id);