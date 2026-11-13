//! HTTP integration tests for the virtual-account (piggy-bank) endpoints:
//!
//!   * `GET /accounts/{id}/balance?vacct_included=...` — the two-balance view.
//!   * `GET /accounts?kind=virtual` — filtering the account list by kind.
//!
//! The scenario seeds a real account (opening 1,000.00) with a 50.00 expense
//! and two set-asides (100.00 and 200.00) into two piggy banks. Both legs of
//! each set-aside carry the piggy's id (`vacct_id`).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use pfm_config::{Config, DatabaseConfig, Engine, SecurityConfig};
use pfm_repo::{DbSettings, Storage};
use pfm_service::{
    AccountService, AuthService, BulkService, CardService, CategoryService, DepositService,
    ExportService, FeeRuleService, ImpersonationService, ImportService, PasswordService,
    PrefsService, RecurrenceService, SharingService, StatsService, TransactionService,
    TransferService, UserAdminService, VirtualAccountService,
};
use tower::ServiceExt;

use pfm_http::csrf::csrf_middleware;
use pfm_http::session::build_session_layer;
use pfm_http::state::AppState;

const TEST_USER: &str = "testuser";
const TEST_PASS: &str = "correct-horse-battery-staple";

/// Build a fully-wired router over an in-memory SQLite store. Seeds the owner,
/// a real account (`real1`), two piggy banks (`pig1`, `pig2`), and the ledger:
/// a 50.00 expense on the real account plus 100.00/200.00 set-asides into the
/// two piggy banks (each set-aside stamps both legs with the piggy's id).
async fn build() -> Router {
    sqlx::any::install_default_drivers();

    let config = Arc::new(Config {
        database: DatabaseConfig {
            engine: Engine::Sqlite,
            sqlite_path: Some(":memory:".into()),
            ..Default::default()
        },
        security: SecurityConfig {
            session_secret: Some(
                "test-secret-key-test-secret-key-test-secret-key-0123456789".into(),
            ),
            rate_limit_login: pfm_config::RateLimitConfig {
                per_min: 100,
                window_s: 60,
            },
            ..Default::default()
        },
        ..Default::default()
    });

    let storage = Storage::connect(&DbSettings {
        engine: pfm_repo::Engine::Sqlite,
        sqlite_path: Some(":memory:".into()),
        sqlite_apply_pragmas: false,
        sqlite_max_connections: 1,
        mariadb_url: None,
        mariadb_max_connections: 0,
        mariadb_min_connections: 0,
    })
    .await
    .unwrap();

    // Real schema, from the migrations the application actually runs — indexes
    // included. The hand-written DDL this replaces defined no indexes at all,
    // so constraint violations that break production went unnoticed here.
    let migrator = sqlx::migrate!("../../migrations");
    storage.migrate(&migrator).await.expect("migrations");

    let pool = storage.pool();

    let pw = PasswordService::new(
        config.security.argon2.memory_kib,
        config.security.argon2.iterations,
        config.security.argon2.parallelism,
    );
    let hash = pw.hash(TEST_PASS).unwrap();

    sqlx::query(
        "INSERT INTO users (id, username, email, password_hash, role, active, locale, default_currency, created_at)
         VALUES ('u1', $1, 'test@example.com', $2, 'user', 1, 'en', 'USD', '2025-01-01T00:00:00Z')",
    )
    .bind(TEST_USER)
    .bind(&hash)
    .execute(pool)
    .await
    .unwrap();

    // Accounts: one real (current) account opening at 1,000.00 and two piggy
    // banks, both slices of the real account.
    sqlx::query(
        "INSERT INTO accounts (id, owner_id, name, kind, start_date, start_balance_cents, active, created_at)
         VALUES ('real1', 'u1', 'corrente', 'current', '2023-01-01', 100000, 1, '2025-01-01T00:00:00Z')",
    )
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO accounts (id, owner_id, name, kind, start_date, start_balance_cents,
         primary_account_id, active, created_at)
         VALUES ('pig1', 'u1', 'tasse', 'virtual', '2023-01-01', 0, 'real1', 1, '2025-01-01T00:00:00Z'),
                ('pig2', 'u1', 'vacanze', 'virtual', '2023-01-01', 0, 'real1', 1, '2025-01-01T00:00:00Z')",
    )
    .execute(pool)
    .await
    .unwrap();

    // Ledger: a 50.00 expense (no piggy) and two set-asides. Each set-aside is a
    // pair: a debit on the real account and a credit on the piggy, both legs
    // tagged with the piggy's id.
    let seed = |id: &str, account: &str, cents: i64, vacct: Option<&str>| {
        sqlx::query(
            "INSERT INTO transactions (id, client_id, account_id, date, cents, description,
             vacct_id, created_by, created_at, updated_at)
             VALUES ($1, $1, $2, '2023-03-01', $3, 'seed', $4, 'u1', '2023-03-01T00:00:00Z', '2023-03-01T00:00:00Z')",
        )
        .bind(id)
        .bind(account)
        .bind(cents)
        .bind(vacct)
        .execute(pool)
    };

    // Ordinary expense on the real account (no piggy).
    sqlx::query(
        "INSERT INTO transactions (id, client_id, account_id, date, cents, description,
         vacct_id, created_by, created_at, updated_at)
         VALUES ('e1', 'e1', 'real1', '2023-02-01', -5000, 'spesa', NULL, 'u1',
                 '2023-03-01T00:00:00Z', '2023-03-01T00:00:00Z')",
    )
    .execute(pool)
    .await
    .unwrap();

    // Set-aside of 100.00 real -> pig1 (both legs tagged pig1).
    seed("s1a", "real1", -10_000, Some("pig1")).await.unwrap();
    seed("s1b", "pig1", 10_000, Some("pig1")).await.unwrap();

    // Set-aside of 200.00 real -> pig2 (both legs tagged pig2).
    seed("s2a", "real1", -20_000, Some("pig2")).await.unwrap();
    seed("s2b", "pig2", 20_000, Some("pig2")).await.unwrap();

    let storage = Arc::new(storage);

    let auth = Arc::new(AuthService::new(&storage, pw));
    let prefs = Arc::new(PrefsService::new(&storage));
    let accounts = Arc::new(AccountService::new(&storage));
    let categories = Arc::new(CategoryService::new(&storage));
    let sharing = Arc::new(SharingService::new(&storage));
    let transactions = Arc::new(TransactionService::new(&storage));
    let transfers = Arc::new(TransferService::new(&storage));
    let cards = Arc::new(CardService::new(&storage));
    let bulk = Arc::new(BulkService::new(&storage));
    let recurrences = Arc::new(RecurrenceService::new(&storage));
    let rules = Arc::new(FeeRuleService::new(&storage));
    let deposits = Arc::new(DepositService::new(&storage));
    let virtuals = Arc::new(VirtualAccountService::new(&storage));
    let stats = Arc::new(StatsService::new(&storage));
    let rate_limiter = Arc::new(pfm_http::rate_limit::RateLimiter::new(
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
        tags: Arc::new(pfm_service::TagService::new(&storage)),
        holidays: Arc::new(pfm_service::HolidayService::new(&storage)),
        recurrences,
        rules,
        virtuals,
        stats,
        import: Arc::new(ImportService::new(&storage)),
        export: Arc::new(ExportService::new(&storage)),
        user_admin: Arc::new(UserAdminService::new(
            &storage,
            pfm_service::PasswordServiceRef::new(
                config.security.argon2.memory_kib,
                config.security.argon2.iterations,
                config.security.argon2.parallelism,
            ),
        )),
        impersonation: Arc::new(ImpersonationService::new(&pfm_repo::SqlxAuthRepo::new(
            &storage,
        ))),
        security: Arc::new(config.security.clone()),
        rate_limiter,
        setup: Arc::new(pfm_service::SetupService::new(
            &pfm_repo::SqlxSetupRepo::new(&storage),
            pfm_service::PasswordService::new(
                config.security.argon2.memory_kib,
                config.security.argon2.iterations,
                config.security.argon2.parallelism,
            ),
        )),
        shutdown_notify: None,
    });

    let session_layer = build_session_layer(&config.security).expect("session layer");

    Router::new()
        .merge(pfm_http::auth_routes::auth_routes(state.clone()))
        .merge(pfm_http::account_routes::account_routes())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            csrf_middleware,
        ))
        .layer(session_layer)
        .with_state(state)
}

fn addr() -> SocketAddr {
    "127.0.0.1:9999".parse().unwrap()
}

fn req_with(method: &str, uri: &str, body: Vec<u8>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    builder = builder.header(header::CONTENT_TYPE, "application/json");
    let mut req = builder.body(Body::from(body)).unwrap();
    req.extensions_mut().insert(ConnectInfo(addr()));
    req
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, String) {
    let res = app.clone().oneshot(req).await.expect("request failed");
    let status = res.status();
    let body = axum::body::to_bytes(res.into_body(), 1_048_576)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&body).into_owned())
}

/// Log in as the seeded owner and return the session cookie.
async fn login(app: &Router) -> String {
    let body = serde_json::to_vec(&serde_json::json!({
        "username": TEST_USER,
        "password": TEST_PASS
    }))
    .unwrap();
    let res = app
        .clone()
        .oneshot(req_with("POST", "/auth/login", body))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "login failed");

    let headers = res.headers().clone();
    let _ = axum::body::to_bytes(res.into_body(), 1_048_576)
        .await
        .unwrap();

    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|c| c.starts_with("pfm_session="))
        .expect("no session cookie")
        .to_string()
}

fn authed_get(uri: &str, session: &str) -> Request<Body> {
    let mut req = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr()));
    req.headers_mut()
        .insert(header::COOKIE, session.parse().unwrap());
    req
}

fn parsed_balance(body_str: &str) -> serde_json::Value {
    serde_json::from_str(body_str).expect("invalid balance JSON")
}

// ─── GET /accounts/{id}/balance ────────────────────────────────────────────────

/// The balance endpoint requires a session.
#[tokio::test]
async fn balance_requires_session() {
    let app = build().await;
    let (status, _) = send(&app, authed_get("/accounts/real1/balance", "")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// With no piggy selected the two balances coincide: 1,000.00 - 50.00 = 950.00
/// (the set-asides are virtual reservations and do not touch the bank balance).
#[tokio::test]
async fn balance_empty_selection_collapses_to_real() {
    let app = build().await;
    let session = login(&app).await;

    let (status, body_str) = send(&app, authed_get("/accounts/real1/balance", &session)).await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let v = parsed_balance(&body_str);
    assert_eq!(v["account_id"], "real1");
    assert_eq!(v["real_cents"], 95_000);
    assert_eq!(
        v["allocated_cents"], 95_000,
        "empty selection => allocated == real"
    );
}

/// Selecting a piggy bank folds its set-aside back into the available balance,
/// dropping it by exactly the amount set aside.
#[tokio::test]
async fn balance_selecting_piggy_reduces_allocated() {
    let app = build().await;
    let session = login(&app).await;

    // Select only pig1 (100.00 set aside).
    let (status, body_str) = send(
        &app,
        authed_get("/accounts/real1/balance?vacct_included=pig1", &session),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    let v = parsed_balance(&body_str);
    assert_eq!(v["real_cents"], 95_000, "physical balance unaffected");
    assert_eq!(
        v["allocated_cents"], 85_000,
        "drops by the 100.00 set-aside"
    );

    // Select both piggies (300.00 total set aside), comma-separated.
    let (status, body_str) = send(
        &app,
        authed_get("/accounts/real1/balance?vacct_included=pig1,pig2", &session),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    let v = parsed_balance(&body_str);
    assert_eq!(v["real_cents"], 95_000);
    assert_eq!(v["allocated_cents"], 65_000, "drops by the 300.00 total");
}

/// A piggy bank's own set-aside is always in its balance, even with an empty
/// selection (its legs are always visible to itself).
#[tokio::test]
async fn balance_piggy_self_view_is_complete() {
    let app = build().await;
    let session = login(&app).await;

    let (status, body_str) = send(&app, authed_get("/accounts/pig1/balance", &session)).await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    let v = parsed_balance(&body_str);
    assert_eq!(v["account_id"], "pig1");
    assert_eq!(v["real_cents"], 0, "piggy has no non-allocated movements");
    assert_eq!(v["allocated_cents"], 10_000, "own set-aside always shown");
}

/// Unknown account → 404, not 500.
#[tokio::test]
async fn balance_missing_account_is_404() {
    let app = build().await;
    let session = login(&app).await;

    let (status, _) = send(&app, authed_get("/accounts/ghost/balance", &session)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ─── GET /accounts?kind=... ───────────────────────────────────────────────────

/// Without a filter the list returns every account (real + piggies).
#[tokio::test]
async fn list_accounts_no_filter_returns_all() {
    let app = build().await;
    let session = login(&app).await;

    let (status, body_str) = send(&app, authed_get("/accounts", &session)).await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    let arr: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(arr.as_array().unwrap().len(), 3);
}

/// `kind=virtual` returns only the piggy banks.
#[tokio::test]
async fn list_accounts_kind_virtual_filters_to_piggies() {
    let app = build().await;
    let session = login(&app).await;

    let (status, body_str) = send(&app, authed_get("/accounts?kind=virtual", &session)).await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    let arr: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let items = arr.as_array().unwrap();
    assert_eq!(items.len(), 2, "only the two piggy banks");
    for item in items {
        assert_eq!(item["kind"], "virtual");
    }
}

/// `kind=current` returns only the real account.
#[tokio::test]
async fn list_accounts_kind_current_filters_to_real() {
    let app = build().await;
    let session = login(&app).await;

    let (status, body_str) = send(&app, authed_get("/accounts?kind=current", &session)).await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    let arr: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let items = arr.as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], "real1");
    assert_eq!(items[0]["kind"], "current");
}

/// An unparseable kind is a client error (400), not a 500.
#[tokio::test]
async fn list_accounts_invalid_kind_is_400() {
    let app = build().await;
    let session = login(&app).await;

    let (status, _) = send(&app, authed_get("/accounts?kind=bogus", &session)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
