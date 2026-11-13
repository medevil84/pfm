-- 0022__amount_colors.sql
-- Per-user colours for positive and negative amounts. Stored as '#rrggbb'.
-- The empty string means "default" (green for positive, red for negative, with
-- the shades the active light/dark theme already uses).
ALTER TABLE user_prefs ADD COLUMN color_pos VARCHAR(7) NOT NULL DEFAULT '';
ALTER TABLE user_prefs ADD COLUMN color_neg VARCHAR(7) NOT NULL DEFAULT '';
