//! Per-user display preferences (the 1:1 `user_prefs` row).
//!
//! Field names mirror the UI mockup and the database columns. Boolean display
//! flags are plain `bool` in the domain; the repository maps them to/from the
//! `INTEGER 0/1` columns the `Any` driver can round-trip.

/// Display / layout preferences for a single user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserPrefs {
    /// Color theme: `"system"`, `"light"` or `"dark"`.
    pub theme: String,
    /// Whether to show the +/- sign on amounts.
    pub show_sign: bool,
    /// Privacy mode (hide balances from shoulder-surfing).
    pub privacy: bool,
    /// Whether to show the monthly balance column.
    pub show_m_saldo: bool,
    /// Number of leading empty days shown before the first movement.
    pub empty_days: i32,
    /// Whether future (not-yet-occurred) movements are shown.
    pub show_future: bool,
    /// Rows shown above the focal point in the movement grid.
    pub rows_above: i32,
    /// Rows shown below the focal point in the movement grid.
    pub rows_below: i32,
    /// Step used when scrolling the movement grid.
    pub rows_step: i32,
    /// Holiday calendar code (`none`, `it`, `de`, `fr`, `es`, `gb`, `us`): which
    /// days count as non-business days for recurrences and card debits.
    pub holiday_calendar: String,
    /// Colour of positive amounts as `#rrggbb`; empty = default (green).
    pub color_pos: String,
    /// Colour of negative amounts as `#rrggbb`; empty = default (red).
    pub color_neg: String,
}

/// Whether `s` is an acceptable amount colour: empty (= default) or `#rrggbb`.
/// The value ends up in a CSS custom property on the client, so anything else
/// is refused instead of stored.
pub fn is_valid_amount_color(s: &str) -> bool {
    s.is_empty()
        || (s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit()))
}

impl Default for UserPrefs {
    /// Defaults mirror the `user_prefs` column defaults in `0002__auth.sql`.
    fn default() -> Self {
        Self {
            theme: "system".to_string(),
            show_sign: true,
            privacy: false,
            show_m_saldo: false,
            empty_days: 0,
            show_future: true,
            rows_above: 2,
            rows_below: 8,
            rows_step: 1,
            holiday_calendar: "it".to_string(),
            color_pos: String::new(),
            color_neg: String::new(),
        }
    }
}
