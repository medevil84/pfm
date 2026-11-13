//! pfm-repo
//!
//! Data access layer. Provides:
//!
//!   * [`Engine`] — the storage engine currently in use.
//!   * [`DbSettings`] — engine-agnostic settings handed to [`Storage::connect`].
//!   * [`Storage`] — a wrapper around a sqlx [`AnyPool`] that knows its engine,
//!     can apply SQLite pragmas on connect, and offers [`Storage::ping`] and
//!     [`Storage::migrate`].
//!   * [`Dialect`] — small helpers for SQL fragments that differ per engine
//!     (e.g. `last_insert_rowid()` vs `last_insert_id()`).
//!   * [`Repository`] — an empty marker trait that every concrete repository
//!     will implement in later steps. Declared now to validate the layering.
//!
//! The concrete repositories (accounts, transactions, ...) arrive in later steps.

pub mod account_rules;
pub mod accounts;
pub mod auth;
pub mod categories;
pub mod dialect;
pub mod holidays;
pub mod import_profiles;
pub mod overrides;
mod pool;
pub mod recurrences;
pub mod setup;
pub mod shares;
pub mod tags;
pub mod transactions;

pub use account_rules::{AccountRulesRepo, SqlxAccountRulesRepo};
pub use accounts::{AccountsRepo, SqlxAccountsRepo};
pub use auth::{AuditRepo, PrefsRepo, SqlxAuthRepo, UsersRepo};
pub use categories::{slugify, CategoriesRepo, SqlxCategoriesRepo};
pub use dialect::Dialect;
pub use holidays::{CalendarInfo, HolidaysRepo, SqlxHolidaysRepo};
pub use import_profiles::{ImportProfilesRepo, SqlxImportProfilesRepo};
pub use overrides::{OverridesRepo, SqlxOverridesRepo};
pub use pool::{DbSettings, Storage};
pub use recurrences::{RecurrencesRepo, SqlxRecurrencesRepo};
pub use setup::{SetupCompleteData, SetupRepo, SqlxSetupRepo};
pub use shares::{SharesRepo, SqlxSharesRepo};
pub use tags::{SqlxTagsRepo, Tag, TagsRepo};
pub use transactions::{ImportRowData, RegisterFilters, SqlxTransactionsRepo, TransactionsRepo};

/// The storage engine backing the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Engine {
    Sqlite,
    MariaDb,
}

/// A data-access repository.
///
/// This is intentionally method-less in the skeleton step: it exists so the
/// `service -> repo` dependency edge compiles and so future repository
/// interfaces have a single, discoverable place to live.
pub trait Repository: Send + Sync {}
