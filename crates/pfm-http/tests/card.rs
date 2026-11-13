//! HTTP integration tests for the credit-card settlement route
//! (`POST /accounts/{id}/card/catch-up`): session/CSRF gating, the
//! response DTO shape, and the service error mappings (not-a-card / missing
//! account).
//!
//! The handler resolves "today" as `Utc::now()`, so each test seeds a raw card
//! movement inside the *current* billing window (computed with the same pure
//! [`pfm_domain::cycle_bounds`] the service uses) and then asserts the charge
//! bills exactly that spend.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use chrono::NaiveDate;
use pfm_config::{Config, DatabaseConfig, Engine, SecurityConfig};
use pfm_domain::cycle_bounds;
use pfm_repo::{DbSettings, Storage};
use pfm_service::VirtualAccountService;
use pfm_service::{
    AccountService, AuthService, BulkService, CardService, CategoryService, DepositService,
    ExportService, FeeRuleService, ImpersonationService, ImportService, PasswordService,
    PrefsService, RecurrenceService, SharingService, StatsService, TransactionService,
    TransferService, UserAdminService,
};
use tower::ServiceExt;

use pfm_http::csrf::csrf_middleware;
use pfm_http::session::build_session_layer;
use pfm_http::state::AppState;

const TEST_USER: &str = "testuser";
const TEST_PASS: &str = "correct-horse-battery-staple";
const LIMIT: i64 = 100_000;
const SPEND: i64 = 12_000;

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

async fn build_test_router() -> (Router, Arc<AppState>, sqlx::AnyPool) {
    sqlx::any::install_default_drivers();

    let config = Config {
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
    };
    let config = Arc::new(config);

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

    let migrator = sqlx::migrate!("../../migrations");
    storage.migrate(&migrator).await.expect("migrations");

    let pool = storage.pool();
    let pool_owned = pool.clone();

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
    let rate_limiter = Arc::new(pfm_http::rate_limit::RateLimiter::new(
        config.security.rate_limit_login.per_min,
        Duration::from_secs(config.security.rate_limit_login.window_s),
    ));
    let deposits = Arc::new(DepositService::new(&storage));
    let virtuals = Arc::new(VirtualAccountService::new(&storage));
    let stats = Arc::new(StatsService::new(&storage));

    let state = Arc::new(AppState {
        auth,
        prefs,
        accounts,
        categories,
        sharing,
        transactions,
        transfers,
        cards,
        bulk,
        tags: Arc::new(pfm_service::TagService::new(&storage)),
        holidays: Arc::new(pfm_service::HolidayService::new(&storage)),
        recurrences,
        rules,
        deposits,
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

    let app = Router::new()
        .route(
            "/healthz",
            axum::routing::get(|| async { (StatusCode::OK, "ok") }),
        )
        .merge(pfm_http::auth_routes::auth_routes(state.clone()))
        .merge(pfm_http::account_routes::account_routes())
        .merge(pfm_http::category_routes::category_routes())
        .merge(pfm_http::transaction_routes::transaction_routes())
        .merge(pfm_http::card_routes::card_routes())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            csrf_middleware,
        ))
        .layer(session_layer)
        .with_state(state.clone());

    (app, state, pool_owned)
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

fn authed_post(uri: &str, session: &str, csrf: &str, body: Vec<u8>) -> Request<Body> {
    let mut req = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-CSRF-Token", csrf)
        .body(Body::from(body))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr()));
    req.headers_mut()
        .insert(header::COOKIE, session.parse().unwrap());
    req
}

fn authed_post_no_csrf(uri: &str, session: &str, body: Vec<u8>) -> Request<Body> {
    let mut req = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr()));
    req.headers_mut()
        .insert(header::COOKIE, session.parse().unwrap());
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

async fn login(app: &Router) -> (String, String) {
    let body = serde_json::to_vec(&serde_json::json!({
        "username": TEST_USER,
        "password": TEST_PASS
    }))
    .unwrap();
    let (status, body_str, headers) = {
        let res = app
            .clone()
            .oneshot(req_with("POST", "/auth/login", body))
            .await
            .unwrap();
        let s = res.status();
        let h = res.headers().clone();
        let b = axum::body::to_bytes(res.into_body(), 1_048_576)
            .await
            .unwrap();
        (s, String::from_utf8_lossy(&b).into_owned(), h)
    };
    assert_eq!(status, StatusCode::OK, "login failed: {body_str}");

    let session_cookie = headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|c| c.starts_with("pfm_session="))
        .expect("no session cookie")
        .to_string();

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let csrf_token = parsed["csrf_token"].as_str().unwrap().to_string();

    (session_cookie, csrf_token)
}

async fn create_current(app: &Router, session: &str, csrf: &str) -> String {
    let body = serde_json::to_vec(&serde_json::json!({
        "name": "checking",
        "kind": "current",
        "icon": "bi-wallet",
        "start_date": "2024-01-01",
        "start_balance_cents": 0
    }))
    .unwrap();
    let (status, body_str) = send(app, authed_post("/accounts", session, csrf, body)).await;
    assert_eq!(status, StatusCode::CREATED, "create current: {body_str}");
    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    parsed["id"].as_str().unwrap().to_string()
}

async fn create_card(app: &Router, session: &str, csrf: &str, primary_id: &str) -> String {
    let body = serde_json::to_vec(&serde_json::json!({
        "name": "plastic",
        "kind": "card",
        "icon": "bi-credit-card",
        "start_date": "2024-01-01",
        "start_balance_cents": 0,
        "limit_cents": LIMIT,
        "charge_day": 1,
        "primary_account_id": primary_id
    }))
    .unwrap();
    let (status, body_str) = send(app, authed_post("/accounts", session, csrf, body)).await;
    assert_eq!(status, StatusCode::CREATED, "create card: {body_str}");
    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    parsed["id"].as_str().unwrap().to_string()
}

async fn seed_raw(pool: &sqlx::AnyPool, account_id: &str, date: NaiveDate, cents: i64) {
    sqlx::query(
        "INSERT INTO transactions \
         (id, client_id, account_id, date, cents, description, created_by, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8)",
    )
    .bind(format!("raw-{}", rand::random::<u64>()))
    .bind("u1")
    .bind(account_id)
    .bind(date.format("%Y-%m-%d").to_string())
    .bind(cents)
    .bind("raw spend")
    .bind("u1")
    .bind("2024-01-01T00:00:00Z")
    .execute(pool)
    .await
    .unwrap();
}

/// Seed one spend of `SPEND` on the card inside the current billing window
/// (charge_day 1), so an end-of-cycle charge bills exactly `SPEND`.
async fn seed_current_cycle_spend(pool: &sqlx::AnyPool, card_id: &str) {
    let today = chrono::Utc::now().date_naive();
    let (_charge_date, cycle_start) =
        cycle_bounds(1, 1, today, d("2024-01-01")).expect("card has a due cycle");
    let spend_date = cycle_start + chrono::Duration::days(5);
    seed_raw(pool, card_id, spend_date, -SPEND).await;
}

#[tokio::test]
async fn catch_up_rejected_without_session() {
    let (app, _, _) = build_test_router().await;
    let body = serde_json::to_vec(&serde_json::json!({ "dry_run": false })).unwrap();
    let (status, _) = send(
        &app,
        req_with("POST", "/accounts/anything/card/catch-up", body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn catch_up_requires_csrf_token() {
    let (app, _, _) = build_test_router().await;
    let (session, _csrf) = login(&app).await;
    let body = serde_json::to_vec(&serde_json::json!({ "dry_run": false })).unwrap();
    let (status, _) = send(
        &app,
        authed_post_no_csrf("/accounts/anything/card/catch-up", &session, body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn catch_up_ok_bills_the_cycle_and_resets_residual() {
    let (app, _state, pool) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let backing = create_current(&app, &session, &csrf).await;
    let card = create_card(&app, &session, &csrf, &backing).await;

    seed_current_cycle_spend(&pool, &card).await;

    let body = serde_json::to_vec(&serde_json::json!({ "dry_run": false })).unwrap();
    let (status, body_str) = send(
        &app,
        authed_post(
            &format!("/accounts/{card}/card/catch-up"),
            &session,
            &csrf,
            body,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(
        parsed["total_charge_cents"], SPEND,
        "charge must bill the cycle's spend"
    );
    let pair_ids = parsed["pair_ids"].as_array().expect("pair_ids array");
    assert!(!pair_ids.is_empty(), "settlement must be written");
    assert_eq!(
        parsed["available_cents"], LIMIT,
        "residual must reset to the full limit after posting"
    );
    assert!(parsed["latest_charge_date"].is_string());
}

#[tokio::test]
async fn catch_up_dry_run_writes_nothing() {
    let (app, _state, pool) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let backing = create_current(&app, &session, &csrf).await;
    let card = create_card(&app, &session, &csrf, &backing).await;

    seed_current_cycle_spend(&pool, &card).await;

    let body = serde_json::to_vec(&serde_json::json!({ "dry_run": true })).unwrap();
    let (status, body_str) = send(
        &app,
        authed_post(
            &format!("/accounts/{card}/card/catch-up"),
            &session,
            &csrf,
            body,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed["total_charge_cents"], SPEND);
    assert!(
        parsed.get("pair_ids").is_none(),
        "dry-run must not write a settlement (pair_ids absent)"
    );
    assert_eq!(
        parsed["available_cents"],
        LIMIT - SPEND,
        "preview reflects the still-outstanding charge"
    );
}

#[tokio::test]
async fn catch_up_rejects_non_card_account() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let current = create_current(&app, &session, &csrf).await;

    let body = serde_json::to_vec(&serde_json::json!({ "dry_run": false })).unwrap();
    let (status, _) = send(
        &app,
        authed_post(
            &format!("/accounts/{current}/card/catch-up"),
            &session,
            &csrf,
            body,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn catch_up_missing_account_404() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let body = serde_json::to_vec(&serde_json::json!({ "dry_run": false })).unwrap();
    let (status, _) = send(
        &app,
        authed_post(
            "/accounts/does-not-exist/card/catch-up",
            &session,
            &csrf,
            body,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
