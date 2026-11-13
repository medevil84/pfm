-- 0012__recurrence_transfer.sql
-- Adds optional "linked account" support to recurrences for recurring transfers.
-- When linked_account_id is set, each occurrence fires two paired transactions
-- (one per account, opposite sign), exactly like a one-time transfer.

ALTER TABLE recurrences ADD COLUMN linked_account_id  VARCHAR(64) NULL;
ALTER TABLE recurrences ADD COLUMN linked_category_id VARCHAR(64) NULL;
