-- 0021__recurrence_materialized_through.sql
-- Last date up to which the scheduler has registered a recurrence's due
-- occurrences. The periodic pass only creates occurrences dated after it, so a
-- row the user deleted on purpose (e.g. a skipped month) is never brought back.
-- NULL = never run: the pass starts after the rule's latest registered row.

ALTER TABLE recurrences ADD COLUMN materialized_through VARCHAR(16) NULL;
