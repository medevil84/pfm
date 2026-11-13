//! pfm-http
//!
//! The HTTP transport layer: axum routing, handlers, request/response
//! serialization, and middleware (session, CSRF, rate limiting).

pub mod account_routes;
pub mod admin_guard;
pub mod auth_routes;
pub mod card_routes;
pub mod category_routes;
pub mod csrf;
pub mod deposit_routes;
pub mod dto;
pub mod error;
pub mod export_routes;
pub mod holiday_routes;
pub mod import_routes;
pub mod localize;
pub mod rate_limit;
pub mod recurrence_routes;
pub mod rule_routes;
pub mod security_headers;
pub mod session;
pub mod session_guard;
pub mod setup_guard;
pub mod setup_routes;
pub mod state;
pub mod stats_routes;
pub mod tag_routes;
pub mod transaction_routes;
pub mod transfer_routes;
pub mod user_admin_routes;
pub mod validate;

use std::sync::Arc;
use std::time::Duration;

use axum::middleware;
use axum::routing::get;
use axum::Router;
use pfm_config::Config;
use pfm_repo::Storage;
use pfm_service::{
    AccountService, AuthService, BulkService, CardService, CategoryService, DepositService,
    ExportService, FeeRuleService, ImpersonationService, ImportService, PasswordService,
    PrefsService, RecurrenceService, SetupService, SharingService, StatsService,
    TransactionService, TransferService, UserAdminService, VirtualAccountService,
};

use crate::session::build_session_layer;
use crate::state::AppState;

/// The operational mode of the application process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    /// Normal operation: full app with all routes.
    Normal,
    /// Setup mode: `/api/setup/*` routes are available.
    /// The `configured` flag tells the UI which page to show:
    ///   - `false` → show the wizard directly
    ///   - `true`  → show the reset confirmation page
    Setup { configured: bool },
    /// Guard mode: DB not configured, no `--setup` flag.
    /// Only the minimal "setup required" page is served.
    Guard,
}

/// Build the fully-wired application router including auth routes, session
/// management, CSRF protection, and health probes.
///
/// In `Setup` mode, the `/api/setup/*` routes are merged in.
/// In `Guard` mode, the router is minimal (only health probes).
///
/// `static_router` carries the static-asset fallback (disk, embedded, or the
/// setup guard page); the strategy depends on the deployment configuration, so
/// the caller builds it. It **must** be passed in here rather than merged by
/// the caller afterwards: `Router::layer` only wraps the routes registered at
/// the time it is called, so a fallback merged after the fact is served with no
/// `Content-Security-Policy`, no `X-Content-Type-Options` and no
/// `Referrer-Policy` — precisely on the HTML pages that need them most.
pub fn build_router(
    storage: Option<Arc<Storage>>,
    config: Arc<Config>,
    mode: AppMode,
    shutdown_notify: Option<Arc<tokio::sync::Notify>>,
    static_router: Option<Router>,
) -> anyhow::Result<Router> {
    // In setup mode every `/api/setup/*` call needs a one-time token that is
    // shown on the operator's terminal only. Those endpoints can wipe the
    // database and create the first admin, and they sit behind no login.
    let setup_token = if matches!(mode, AppMode::Setup { .. }) {
        let token = session::generate_csrf_token();
        eprintln!(
            "\nSETUP TOKEN (required by the setup wizard, valid for this run only):\n  {token}\n\
             Open the wizard as  http://<server-address>:{}/#t={token}\n",
            config.server.port
        );
        Some(token)
    } else {
        None
    };
    build_router_with_setup_token(
        storage,
        config,
        mode,
        shutdown_notify,
        static_router,
        setup_token,
    )
}

/// Like [`build_router`] but with the setup token supplied by the caller (the
/// integration tests need to know it). Ignored outside [`AppMode::Setup`].
pub fn build_router_with_setup_token(
    storage: Option<Arc<Storage>>,
    config: Arc<Config>,
    mode: AppMode,
    shutdown_notify: Option<Arc<tokio::sync::Notify>>,
    static_router: Option<Router>,
    setup_token: Option<String>,
) -> anyhow::Result<Router> {
    let security = Arc::new(config.security.clone());
    for warning in security.warnings() {
        tracing::warn!("{warning}");
    }

    // In Guard mode we don't need a storage-backed router at all — but the
    // guard page still goes through the security headers.
    if matches!(mode, AppMode::Guard) {
        let mut app = Router::new()
            .route("/healthz", get(healthz))
            .route("/readyz", get(readyz_guard));
        if let Some(static_router) = static_router {
            app = app.merge(static_router);
        }
        let app = app
            .layer(middleware::from_fn_with_state(
                security.clone(),
                crate::security_headers::security_headers,
            ))
            .layer(middleware::from_fn(crate::localize::localize_errors));
        return Ok(app);
    }

    let (
        auth,
        prefs,
        accounts,
        categories,
        sharing,
        transactions,
        transfers,
        cards,
        deposits,
        bulk,
        tags,
        holidays,
        recurrences,
        rules,
        virtuals,
        stats,
        import,
        export,
        user_admin,
        impersonation,
        setup,
    ) = match &storage {
        Some(s) => {
            let pw = PasswordService::new(
                config.security.argon2.memory_kib,
                config.security.argon2.iterations,
                config.security.argon2.parallelism,
            );
            let pw_ref = pfm_service::PasswordServiceRef::new(
                config.security.argon2.memory_kib,
                config.security.argon2.iterations,
                config.security.argon2.parallelism,
            );
            (
                Arc::new(AuthService::new(s, pw.clone())),
                Arc::new(PrefsService::new(s)),
                Arc::new(AccountService::new(s)),
                Arc::new(CategoryService::new(s)),
                Arc::new(SharingService::new(s)),
                Arc::new(TransactionService::new(s)),
                Arc::new(TransferService::new(s)),
                Arc::new(CardService::new(s)),
                Arc::new(DepositService::new(s)),
                Arc::new(BulkService::new(s)),
                Arc::new(pfm_service::TagService::new(s)),
                Arc::new(pfm_service::HolidayService::new(s)),
                Arc::new(RecurrenceService::new(s)),
                Arc::new(FeeRuleService::new(s)),
                Arc::new(VirtualAccountService::new(s)),
                Arc::new(StatsService::new(s)),
                Arc::new(ImportService::new(s)),
                Arc::new(ExportService::new(s)),
                Arc::new(UserAdminService::new(s, pw_ref)),
                Arc::new(ImpersonationService::new(&pfm_repo::SqlxAuthRepo::new(s))),
                Arc::new(SetupService::new(&pfm_repo::SqlxSetupRepo::new(s), pw)),
            )
        }
        None => {
            return Err(anyhow::anyhow!("storage is required for auth routes"));
        }
    };

    let rate_limiter = Arc::new(crate::rate_limit::RateLimiter::new(
        config.security.rate_limit_login.per_min,
        Duration::from_secs(config.security.rate_limit_login.window_s),
    ));

    let state = Arc::new(AppState {
        auth,
        prefs,
        accounts,
        categories,
        sharing,
        transactions,
        transfers,
        cards,
        deposits,
        bulk,
        tags,
        holidays,
        recurrences,
        rules,
        virtuals,
        stats,
        import,
        export,
        user_admin,
        impersonation,
        setup,
        security: security.clone(),
        rate_limiter,
        shutdown_notify,
    });

    // Background alignment of recurrences, fee rules and card settlements. The
    // startup pass always runs; the periodic loop only when enabled in config.
    // Future recurrence occurrences are only forecast; once due they must be
    // registered, as must rule rows and card settlements.
    {
        let recs_svc = Arc::clone(&state.recurrences);
        let rules_svc = Arc::clone(&state.rules);
        let cards_svc = Arc::clone(&state.cards);
        let enabled = config.scheduler.enabled;
        let interval_secs = config.scheduler.interval_secs;
        let notify = state.shutdown_notify.clone();

        tokio::spawn(async move {
            tracing::info!("scheduler: startup alignment pass");
            let n = align_all(&recs_svc, &rules_svc, &cards_svc).await;
            tracing::info!(
                "scheduler: alignment complete, {n} card(s) settled, periodic={enabled}, interval={interval_secs}s"
            );
            if !enabled {
                return;
            }

            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
            ticker.tick().await; // discard the immediate first tick (already ran above)

            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        tracing::debug!("scheduler: periodic alignment pass");
                        let n = align_all(&recs_svc, &rules_svc, &cards_svc).await;
                        if n > 0 {
                            tracing::debug!("scheduler: {} card(s) settled", n);
                        }
                    }
                    _ = async {
                        if let Some(ref n) = notify { n.notified().await }
                        else { std::future::pending::<()>().await }
                    } => {
                        tracing::info!("scheduler: shutdown signal received");
                        break;
                    }
                }
            }
        });
    }

    let session_layer = build_session_layer(&config.security)?;

    // Admin routes wrapped with the admin_guard middleware.
    let admin_router = crate::user_admin_routes::admin_routes()
        .merge(crate::holiday_routes::holiday_admin_routes())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::admin_guard::admin_guard,
        ));

    // All API routes are served under the `/api` prefix so they match the
    // frontend's `const API = '/api'` convention. Without this, every API
    // call (e.g. `/api/auth/login`) would not match a route and would fall
    // through to the static-asset fallback, which returns `index.html`
    // (HTML) instead of JSON.
    //
    // The setup routes already include the full `/api/setup/*` path in their
    // route definitions, so they are merged at the top level below (not
    // nested) to avoid a doubled `/api/api/...` prefix.
    let api = Router::new()
        .merge(auth_routes::auth_routes(state.clone()))
        .merge(account_routes::account_routes())
        .merge(category_routes::category_routes())
        .merge(tag_routes::tag_routes())
        .merge(holiday_routes::holiday_routes())
        .merge(transaction_routes::transaction_routes())
        .merge(transfer_routes::transfer_routes())
        .merge(card_routes::card_routes())
        .merge(deposit_routes::deposit_routes())
        .merge(recurrence_routes::recurrence_routes())
        .merge(rule_routes::rule_routes())
        .merge(stats_routes::stats_routes())
        .merge(import_routes::import_routes())
        .merge(export_routes::export_routes())
        .merge(admin_router);

    let mut app = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz));

    if matches!(mode, AppMode::Setup { .. }) {
        // Setup mode serves the wizard and nothing else: the regular API (login,
        // accounts, …) stays unmounted so a half-initialised instance cannot be
        // driven through it. The /api/setup/* routes already carry the full
        // path including the /api prefix.
        let token =
            setup_token.ok_or_else(|| anyhow::anyhow!("setup mode requires a setup token"))?;
        app = app.merge(setup_routes::setup_routes(Arc::new(token)));
    } else {
        app = app.nest("/api", api);
    }

    // `with_state` first: it turns `Router<Arc<AppState>>` into `Router<()>`,
    // which is the type the caller's static fallback has. The middleware below
    // carries its own state via `from_fn_with_state`, so it does not depend on
    // the router's state type and can be applied afterwards.
    let app = app.with_state(state.clone());

    // Merged *before* the layers below, so the static assets and HTML pages
    // are covered by the same middleware stack as the API. GETs pass through
    // the CSRF middleware untouched (it only inspects POST/PUT/DELETE).
    let app = match static_router {
        Some(static_router) => app.merge(static_router),
        None => app,
    };

    let app = app
        .layer(middleware::from_fn_with_state(
            state.clone(),
            csrf::csrf_middleware,
        ))
        // Outside CSRF, inside the session layer: a disabled / re-passworded
        // user is turned away (401) before anything else runs.
        .layer(middleware::from_fn_with_state(
            state.clone(),
            session_guard::session_guard,
        ))
        .layer(session_layer)
        .layer(middleware::from_fn_with_state(
            security.clone(),
            crate::security_headers::security_headers,
        ))
        .layer(middleware::from_fn(crate::localize::localize_errors));

    Ok(app)
}

/// Liveness probe.
async fn healthz() -> (axum::http::StatusCode, axum::Json<serde_json::Value>) {
    (
        axum::http::StatusCode::OK,
        axum::Json(serde_json::json!({ "status": "ok" })),
    )
}

/// Readiness probe (normal mode).
///
/// Actually touches the database instead of reporting `ready` unconditionally:
/// `is_configured` reads the `setup_state` table, so a successful response
/// proves both that the pool answers and that the schema is present. An
/// always-`ready` probe is worse than no probe, because an orchestrator will
/// keep routing traffic to a process whose database has gone away.
async fn readyz(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Result<axum::Json<serde_json::Value>, (axum::http::StatusCode, axum::Json<serde_json::Value>)>
{
    match state.setup.is_configured().await {
        Ok(_) => Ok(axum::Json(serde_json::json!({ "status": "ready" }))),
        Err(err) => {
            tracing::warn!("readiness probe failed: {err}");
            Err((
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                axum::Json(serde_json::json!({ "status": "unavailable" })),
            ))
        }
    }
}

/// Readiness probe (guard mode – no AppState).
async fn readyz_guard() -> (axum::http::StatusCode, axum::Json<serde_json::Value>) {
    (
        axum::http::StatusCode::OK,
        axum::Json(serde_json::json!({ "status": "guard" })),
    )
}

/// One alignment pass. Due recurrence occurrences are registered first (rules
/// and card cycles must see them), then rules (so card settlements see their
/// rows on the backing account), then card settlements, then rules again: a
/// rule whose condition depends on the balance (e.g. stamp duty on the average
/// balance) must see the settlement debits that were just written.
async fn align_all(
    recs: &pfm_service::RecurrenceService,
    rules: &pfm_service::FeeRuleService,
    cards: &pfm_service::CardService,
) -> usize {
    let r = recs.materialize_all_due().await;
    if r > 0 {
        tracing::info!("scheduler: {r} recurrence occurrence(s) registered");
    }
    rules.materialize_all_pending().await;
    let n = cards.catch_up_all(chrono::Utc::now().date_naive()).await;
    if n > 0 {
        rules.materialize_all_pending().await;
    }
    n
}
