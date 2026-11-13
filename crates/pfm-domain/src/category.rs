//! The category entity and its kind.
//!
//! A category buckets movements. Its `key` is a stable slug generated from the
//! name ONLY at creation time and never changes on rename, so movement
//! references by category id/key stay intact. `kind` (out vs. in) is fixed at
//! creation. The two seeded `is_fallback` categories are not deletable.

/// The direction a category applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CategoryKind {
    /// An outflow category (expense).
    Out,
    /// An inflow category (income).
    In,
}

impl CategoryKind {
    /// Stable string form used on the wire and in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            CategoryKind::Out => "out",
            CategoryKind::In => "in",
        }
    }

    /// Parse a kind from its string form. `None` for unknown values.
    pub fn parse(s: &str) -> Option<CategoryKind> {
        Some(match s {
            "out" => CategoryKind::Out,
            "in" => CategoryKind::In,
            _ => return None,
        })
    }
}

impl std::fmt::Display for CategoryKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A persisted category row (the full entity, as stored in `categories`).
#[derive(Debug, Clone)]
pub struct Category {
    /// Opaque identifier (UUID string, application-generated).
    pub id: String,
    /// Stable slug, generated from the name only at creation time.
    pub key: String,
    /// Display name.
    pub name: String,
    /// Emoji / icon shown in the UI.
    pub emoji: String,
    pub kind: CategoryKind,
    /// Monthly budget in cents, if any.
    pub budget_cents: Option<i64>,
    /// Whether this is a seeded, non-deletable fallback category.
    pub is_fallback: bool,
    /// Creation timestamp, RFC-3339 UTC.
    pub created_at: String,
    /// The user who created it. `None` = shared: the seeded fallbacks and rows
    /// that predate per-user ownership; only an administrator may change those.
    pub owner_id: Option<String>,
}
