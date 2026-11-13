-- 0013__card_settlement.sql
-- Add card settlement tracking fields to accounts.
-- last_settled_date: watermark for incremental settlement (default 1900-01-01).
-- Only meaningful for card accounts; ignored for other kinds.

ALTER TABLE accounts ADD COLUMN last_settled_date VARCHAR(16) NOT NULL DEFAULT '1900-01-01';