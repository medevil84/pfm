//! pfm-i18n
//!
//! Locale negotiation and internationalization primitives.
//!
//! This crate is deliberately dependency-light (`serde` + `serde_json`) so that
//! both the configuration layer and the HTTP layer can select on [`Locale`] and
//! resolve message texts without pulling in the web stack.
//!
//! It provides:
//! * the [`Locale`] enum and a best-effort [`negotiate`] implementation
//!   (parsing the `Accept-Language` header);
//! * a small embedded message catalogue (the `catalogs/it.json` and
//!   `catalogs/en.json` files, compiled in via `include_str!`) and the
//!   [`translate`] / [`translate_with`] lookup helpers.
//!
//! The catalogue stores **stable codes** (e.g. `errors.account_not_found`) as
//! keys. Every user-facing string that the backend emits is keyed by such a
//! code; the human-readable text for a given locale lives **only** here, never
//! hard-coded in the HTTP handlers.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;

/// A supported UI / output locale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Locale {
    /// English.
    En,
    /// Italian.
    It,
}

impl Default for Locale {
    /// The default locale is Italian (matches the UI and DB defaults).
    fn default() -> Self {
        Locale::It
    }
}

impl Locale {
    /// The BCP-47 language tag used for `Intl.*` formatting and `<html lang>`.
    pub fn tag(self) -> &'static str {
        match self {
            Locale::En => "en",
            Locale::It => "it",
        }
    }

    /// Parse a single locale tag (e.g. `"en"`, `"it-IT"`) into a [`Locale`].
    ///
    /// Matching is case-insensitive and tolerant of a subtag suffix: the primary
    /// language subtag (the part before any `-` or `_`) is compared in
    /// lower-case. Returns `None` when the primary subtag is not supported.
    pub fn parse(tag: &str) -> Option<Locale> {
        let primary = tag
            .split(['-', '_'])
            .next()
            .unwrap_or(tag)
            .to_ascii_lowercase();
        match primary.as_str() {
            "en" => Some(Locale::En),
            "it" => Some(Locale::It),
            _ => None,
        }
    }
}

/// Select the best [`Locale`] from an `Accept-Language` header value.
///
/// The header is expected to be a comma-separated list of locale tags, optionally
/// with `q=` quality values (e.g. `"it-IT,it;q=0.9,en;q=0.8"`). Entries are
/// ordered by their declared quality (descending, then by original order) and the
/// first tag that maps to a supported `supported` locale is returned.
///
/// Returns `default` when the header is absent, empty, or matches no supported
/// locale.
pub fn negotiate(header: Option<&str>, supported: &[Locale], default: Locale) -> Locale {
    let Some(header) = header.map(str::trim) else {
        return default;
    };
    if header.is_empty() {
        return default;
    }

    // Parse into (quality, original_index, locale_tag) tuples.
    let mut entries: Vec<(f64, usize, String)> = Vec::new();
    for (idx, part) in header.split(',').enumerate() {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let mut quality = 1.0f64;
        let mut tag: Option<&str> = None;
        for param in part.split(';') {
            let param = param.trim();
            if param.is_empty() {
                continue;
            }
            if let Some(value) = param.strip_prefix("q=") {
                if let Ok(q) = value.parse::<f64>() {
                    quality = q;
                }
            } else if tag.is_none() {
                tag = Some(param);
            }
        }
        let tag = tag.unwrap_or(part);
        entries.push((quality, idx, tag.to_string()));
    }

    // Highest quality first; stable tie-break by original position.
    entries.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
    });

    for (_, _, tag) in entries {
        if let Some(locale) = Locale::parse(&tag) {
            if supported.contains(&locale) {
                return locale;
            }
        }
    }

    default
}

// ── Embedded catalogue ─────────────────────────────────────────────────────────

type Catalog = HashMap<String, String>;

/// The two supported locales, in the order they are wired into lookups.
pub const SUPPORTED: [Locale; 2] = [Locale::It, Locale::En];

static IT: OnceLock<Catalog> = OnceLock::new();
static EN: OnceLock<Catalog> = OnceLock::new();

fn parse_catalog(raw: &str) -> Catalog {
    // The files are generated as flat `{ code: text }` objects. A malformed
    // catalogue is a build-time defect, so a panic here is acceptable.
    serde_json::from_str(raw).expect("i18n catalog must be a flat JSON object of strings")
}

fn catalog(locale: Locale) -> &'static Catalog {
    match locale {
        Locale::It => IT.get_or_init(|| parse_catalog(include_str!("../catalogs/it.json"))),
        Locale::En => EN.get_or_init(|| parse_catalog(include_str!("../catalogs/en.json"))),
    }
}

/// Resolve a stable `code` to its human-readable text in `locale`.
///
/// Falls back to the Italian catalogue when the requested locale has no entry,
/// and finally to the `code` itself (so a missing translation can never panic
/// or render empty).
pub fn translate(code: &str, locale: Locale) -> String {
    if let Some(text) = catalog(locale).get(code) {
        return text.clone();
    }
    if locale != Locale::It {
        if let Some(text) = catalog(Locale::It).get(code) {
            return text.clone();
        }
    }
    code.to_string()
}

/// Like [`translate`], then appends a free-form `args` suffix (e.g. a list of
/// account names or a service-generated validation message) when non-empty.
///
/// The suffix is *not* translated: it is produced by the service layer in a
/// locale-independent form (identifiers, numbers), which keeps the catalogue
/// stable and the DB locale-free.
pub fn translate_with(code: &str, locale: Locale, args: Option<&str>) -> String {
    let base = translate(code, locale);
    match args {
        Some(a) if !a.trim().is_empty() => format!("{base} {a}"),
        _ => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUPPORTED: [Locale; 2] = [Locale::En, Locale::It];

    #[test]
    fn negotiates_primary_tag() {
        assert_eq!(negotiate(Some("it"), &SUPPORTED, Locale::En), Locale::It);
        assert_eq!(negotiate(Some("en-US"), &SUPPORTED, Locale::En), Locale::En);
    }

    #[test]
    fn respects_quality_order() {
        assert_eq!(
            negotiate(Some("en,it;q=0.9"), &SUPPORTED, Locale::It),
            Locale::En
        );
        assert_eq!(
            negotiate(Some("en;q=0.5,it;q=0.9"), &SUPPORTED, Locale::En),
            Locale::It
        );
    }

    #[test]
    fn falls_back_to_default() {
        assert_eq!(negotiate(None, &SUPPORTED, Locale::It), Locale::It);
        assert_eq!(negotiate(Some("de"), &SUPPORTED, Locale::En), Locale::En);
        assert_eq!(negotiate(Some("  "), &SUPPORTED, Locale::En), Locale::En);
    }

    #[test]
    fn unsupported_filter_respected() {
        let only_en = [Locale::En];
        assert_eq!(negotiate(Some("it"), &only_en, Locale::En), Locale::En);
    }

    #[test]
    fn locale_default_is_italian() {
        assert_eq!(Locale::default(), Locale::It);
    }

    #[test]
    fn tags_match_bcp47() {
        assert_eq!(Locale::It.tag(), "it");
        assert_eq!(Locale::En.tag(), "en");
    }

    #[test]
    fn translate_returns_locale_text() {
        assert!(translate("errors.account_not_found", Locale::It).contains('o'));
        let en = translate("errors.account_not_found", Locale::En);
        assert_eq!(en, "Account not found");
    }

    #[test]
    fn translate_falls_back_to_code() {
        assert_eq!(translate("no.such.code", Locale::En), "no.such.code");
    }

    #[test]
    fn translate_with_appends_args() {
        let s = translate_with("errors.reassignment_required", Locale::En, Some("a, b"));
        assert!(s.starts_with("User has accounts"));
        assert!(s.ends_with("a, b"));
    }

    #[test]
    fn translate_with_ignores_empty_args() {
        assert_eq!(
            translate_with("errors.account_not_found", Locale::En, Some("  ")),
            "Account not found"
        );
    }
}
