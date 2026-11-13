-- 0019__rule_prorata_days.sql
-- Per-rule flag: charge a fixed annual amount pro-rata on each period's actual
-- days (stamp duty), debited on the first business day after the period ends.
-- Existing fixed annual rules on a sub-annual cadence keep the behaviour they
-- were created for (the stamp-duty preset), so they are switched on.
ALTER TABLE account_rules ADD COLUMN prorata_days INTEGER NOT NULL DEFAULT 0;
UPDATE account_rules SET prorata_days = 1
 WHERE is_annual = 1 AND calc_type = 'fixed' AND cadence <> 'annual';
