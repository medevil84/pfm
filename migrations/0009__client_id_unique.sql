-- 0009__client_id_unique.sql
-- Add a unique index on transactions.client_id to enable idempotent
-- offline synchronization: the same client_id sent twice (double-tap,
-- network retry) resolves to a single row instead of a duplicate.
--
-- Dialect-neutral: CREATE UNIQUE INDEX works on both SQLite and MariaDB.

CREATE UNIQUE INDEX IF NOT EXISTS idx_transactions_client_id ON transactions (client_id);