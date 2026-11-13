-- 0015__card_settlement_category.sql
-- Card settlement legs used to be written without a category. Classify the
-- existing ones under the fallback category matching their sign, so category
-- filters and statistics include them.
UPDATE transactions
   SET category_id = (SELECT id FROM categories WHERE is_fallback = 1 AND kind = 'out' LIMIT 1)
 WHERE pair_id LIKE 'cc:%' AND category_id IS NULL AND cents < 0;

UPDATE transactions
   SET category_id = (SELECT id FROM categories WHERE is_fallback = 1 AND kind = 'in' LIMIT 1)
 WHERE pair_id LIKE 'cc:%' AND category_id IS NULL AND cents > 0;

-- Card settlement pair ids are now keyed by charge month (cc:<card>:YYYY-MM)
-- instead of the exact date, so a charge-day change updates the existing
-- settlement rather than creating a second one. Strip the trailing "-DD".
UPDATE transactions
   SET pair_id = substr(pair_id, 1, length(pair_id) - 3)
 WHERE pair_id LIKE 'cc:%' AND length(pair_id) > 3 AND substr(pair_id, length(pair_id) - 2, 1) = '-'
   AND substr(pair_id, length(pair_id) - 5, 1) = '-';

-- Deposit interest legs are classified under the "in" fallback category too.
UPDATE transactions
   SET category_id = (SELECT id FROM categories WHERE is_fallback = 1 AND kind = 'in' LIMIT 1)
 WHERE pair_id LIKE 'dep:%' AND category_id IS NULL;
