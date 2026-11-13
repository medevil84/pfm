-- 0017__holiday_calendar.sql
-- Per-user holiday calendar used to decide which days are business days when
-- shifting recurrences and card debit dates. Codes: none, it, de, fr, es, gb, us.
ALTER TABLE user_prefs ADD COLUMN holiday_calendar VARCHAR(8) NOT NULL DEFAULT 'it';
