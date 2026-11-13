-- 0011__account_sort_order.sql
-- Adds a user-controlled display order for accounts.
-- Existing rows all default to 0; the secondary ORDER BY name keeps them stable.

ALTER TABLE accounts ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0;
