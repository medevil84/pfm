//! Shared application state for HTTP handlers.

use std::sync::Arc;

use pfm_config::SecurityConfig;
use pfm_service::{
    AccountService, AuthService, BulkService, CardService, CategoryService, DepositService,
    ExportService, FeeRuleService, ImpersonationService, ImportService, PrefsService,
    RecurrenceService, SetupService, SharingService, StatsService, TagService, TransactionService,
    TransferService, UserAdminService, VirtualAccountService,
};

use crate::rate_limit::RateLimiter;

/// Cloneable state shared across all handlers.
#[derive(Clone)]
pub struct AppState {
    pub auth: Arc<AuthService>,
    pub prefs: Arc<PrefsService>,
    pub accounts: Arc<AccountService>,
    pub categories: Arc<CategoryService>,
    pub sharing: Arc<SharingService>,
    pub transactions: Arc<TransactionService>,
    pub transfers: Arc<TransferService>,
    pub cards: Arc<CardService>,
    pub deposits: Arc<DepositService>,
    pub bulk: Arc<BulkService>,
    pub tags: Arc<TagService>,
    pub holidays: Arc<pfm_service::HolidayService>,
    pub recurrences: Arc<RecurrenceService>,
    pub rules: Arc<FeeRuleService>,
    pub virtuals: Arc<VirtualAccountService>,
    pub stats: Arc<StatsService>,
    pub import: Arc<ImportService>,
    pub export: Arc<ExportService>,
    pub user_admin: Arc<UserAdminService>,
    pub impersonation: Arc<ImpersonationService>,
    pub setup: Arc<SetupService>,
    pub security: Arc<SecurityConfig>,
    pub rate_limiter: Arc<RateLimiter>,
    /// When `Some`, calling `notify_waiters()` triggers a graceful shutdown
    /// of the process. Used by the setup-completion endpoint.
    pub shutdown_notify: Option<Arc<tokio::sync::Notify>>,
}
