-- 0014__card_closing_day.sql
-- Add the accounting closing day to accounts, distinct from the existing
-- charge day. For a card account:
--   - closing_day: the day of the month the statement cycle closes.
--   - charge_day: the day of the month the backing account is debited
--     (already existed). It is always considered subsequent to closing_day:
--     same month when charge_day >= closing_day, otherwise the next month.
--
-- Existing card accounts are backfilled with closing_day = charge_day, which
-- reproduces their previous single-day behavior exactly (cycle bounded by
-- that day, charged the same day).
ALTER TABLE accounts ADD COLUMN closing_day INTEGER;

UPDATE accounts SET closing_day = charge_day WHERE kind = 'card' AND charge_day IS NOT NULL;
