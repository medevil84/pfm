//! HTTP integration tests for the accounts routes.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use pfm_config::{Config, DatabaseConfig, Engine, SecurityConfig};
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

async fn build_test_router() -> (Router, Arc<AppState>) {
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
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            csrf_middleware,
        ))
        .layer(session_layer)
        .with_state(state.clone());

    (app, state)
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

fn authed_put(uri: &str, session: &str, csrf: &str, body: Vec<u8>) -> Request<Body> {
    let mut req = Request::builder()
        .method("PUT")
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

fn authed_delete(uri: &str, session: &str, csrf: &str) -> Request<Body> {
    let mut req = Request::builder()
        .method("DELETE")
        .uri(uri)
        .header("X-CSRF-Token", csrf)
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr()));
    req.headers_mut()
        .insert(header::COOKIE, session.parse().unwrap());
    req
}

// ─── accounts: requires auth ──────────────────────────────────────────────────

#[tokio::test]
async fn accounts_requires_session() {
    let (app, _) = build_test_router().await;
    let (status, _) = send(&app, authed_get("/accounts", "")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ─── accounts: create ─────────────────────────────────────────────────────────

#[tokio::test]
async fn create_current_account() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "name": "conto principale",
        "kind": "current",
        "icon": "bi-bank",
        "iban": "IT60X0542811101000000123456",
        "start_date": "2024-01-15",
        "start_balance_cents": 150000
    }))
    .unwrap();

    let (status, body_str) = send(&app, authed_post("/accounts", &session, &csrf, body)).await;
    assert_eq!(status, StatusCode::CREATED, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert!(!parsed["id"].as_str().unwrap().is_empty());
    assert_eq!(parsed["name"], "conto principale");
    assert_eq!(parsed["kind"], "current");
    assert_eq!(parsed["active"], true);
}

#[tokio::test]
async fn create_card_account_requires_limit_and_charge_day() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    // Missing limit_cents
    let body = serde_json::to_vec(&serde_json::json!({
        "name": "mastercard",
        "kind": "card",
        "icon": "bi-credit-card",
        "start_date": "2024-06-01",
        "start_balance_cents": 0,
        "charge_day": 25
    }))
    .unwrap();

    let (status, body_str) = send(&app, authed_post("/accounts", &session, &csrf, body)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body_str}");
    assert!(body_str.contains("limit_cents"));

    // Missing charge_day
    let body = serde_json::to_vec(&serde_json::json!({
        "name": "mastercard",
        "kind": "card",
        "icon": "bi-credit-card",
        "start_date": "2024-06-01",
        "start_balance_cents": 0,
        "limit_cents": 500000
    }))
    .unwrap();

    let (status, body_str) = send(&app, authed_post("/accounts", &session, &csrf, body)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body_str}");
    assert!(body_str.contains("charge_day"));
}

#[tokio::test]
async fn create_card_account_valid() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "name": "mastercard",
        "kind": "card",
        "icon": "bi-credit-card",
        "start_date": "2024-06-01",
        "start_balance_cents": 0,
        "limit_cents": 500000,
        "charge_day": 25
    }))
    .unwrap();

    let (status, body_str) = send(&app, authed_post("/accounts", &session, &csrf, body)).await;
    assert_eq!(status, StatusCode::CREATED, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed["kind"], "card");
    assert_eq!(parsed["limit_cents"], 500000);
    assert_eq!(parsed["charge_day"], 25);
}

#[tokio::test]
async fn create_deposit_requires_rate_and_primary() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    // Missing rate_pct_bps
    let body = serde_json::to_vec(&serde_json::json!({
        "name": "deposito",
        "kind": "deposit",
        "icon": "bi-piggy-bank",
        "start_date": "2024-01-01",
        "start_balance_cents": 100000,
        "primary_account_id": "primary-acc-id"
    }))
    .unwrap();

    let (status, body_str) = send(&app, authed_post("/accounts", &session, &csrf, body)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body_str}");
    assert!(body_str.contains("rate_pct_bps"));

    // Missing primary_account_id
    let body = serde_json::to_vec(&serde_json::json!({
        "name": "deposito",
        "kind": "deposit",
        "icon": "bi-piggy-bank",
        "start_date": "2024-01-01",
        "start_balance_cents": 100000,
        "rate_pct_bps": 300
    }))
    .unwrap();

    let (status, body_str) = send(&app, authed_post("/accounts", &session, &csrf, body)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body_str}");
    assert!(body_str.contains("primary_account_id"));
}

#[tokio::test]
async fn create_virtual_with_goal() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "name": "risparmio casa",
        "kind": "virtual",
        "icon": "bi-house",
        "start_date": "2025-01-01",
        "start_balance_cents": 50000,
        "goal_cents": 1000000
    }))
    .unwrap();

    let (status, body_str) = send(&app, authed_post("/accounts", &session, &csrf, body)).await;
    assert_eq!(status, StatusCode::CREATED, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed["kind"], "virtual");
    assert_eq!(parsed["goal_cents"], 1000000);
}

#[tokio::test]
async fn create_invalid_kind_rejected() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "name": "test",
        "kind": "savings",
        "icon": "bi-wallet",
        "start_date": "2024-01-01",
        "start_balance_cents": 0
    }))
    .unwrap();

    let (status, body_str) = send(&app, authed_post("/accounts", &session, &csrf, body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body_str}");
}

// ─── accounts: list ───────────────────────────────────────────────────────────

#[tokio::test]
async fn list_accounts_empty() {
    let (app, _) = build_test_router().await;
    let (session, _) = login(&app).await;

    let (status, body_str) = send(&app, authed_get("/accounts", &session)).await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert!(parsed.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn list_returns_created_accounts() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "name": "conto A",
        "kind": "current",
        "icon": "bi-bank",
        "start_date": "2024-01-01",
        "start_balance_cents": 0
    }))
    .unwrap();
    send(&app, authed_post("/accounts", &session, &csrf, body)).await;

    let (status, body_str) = send(&app, authed_get("/accounts", &session)).await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let arr = parsed.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["name"], "conto A");
}

// ─── accounts: get ────────────────────────────────────────────────────────────

#[tokio::test]
async fn get_account_by_id() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "name": "my account",
        "kind": "current",
        "icon": "bi-bank",
        "start_date": "2024-03-10",
        "start_balance_cents": 42000
    }))
    .unwrap();
    let (status, body_str) = send(&app, authed_post("/accounts", &session, &csrf, body)).await;
    assert_eq!(status, StatusCode::CREATED, "body: {body_str}");
    let created: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let id = created["id"].as_str().unwrap().to_string();

    let (status, body_str) = send(&app, authed_get(&format!("/accounts/{id}"), &session)).await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed["id"], id);
    assert_eq!(parsed["name"], "my account");
}

#[tokio::test]
async fn get_missing_account_returns_404() {
    let (app, _) = build_test_router().await;
    let (session, _) = login(&app).await;

    let (status, _) = send(&app, authed_get("/accounts/nonexistent", &session)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ─── accounts: update ─────────────────────────────────────────────────────────

#[tokio::test]
async fn update_account_name_and_active() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "name": "original",
        "kind": "current",
        "icon": "bi-bank",
        "start_date": "2024-01-01",
        "start_balance_cents": 0
    }))
    .unwrap();
    let (_, body_str) = send(&app, authed_post("/accounts", &session, &csrf, body)).await;
    let created: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let id = created["id"].as_str().unwrap().to_string();

    let update_body = serde_json::to_vec(&serde_json::json!({
        "name": "renamed",
        "active": false
    }))
    .unwrap();
    let (status, body_str) = send(
        &app,
        authed_put(&format!("/accounts/{id}"), &session, &csrf, update_body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed["name"], "renamed");
    assert_eq!(parsed["active"], false);
}

#[tokio::test]
async fn update_missing_account_returns_404() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let body = serde_json::to_vec(&serde_json::json!({ "name": "x" })).unwrap();
    let (status, _) = send(&app, authed_put("/accounts/ghost", &session, &csrf, body)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ─── accounts: delete ─────────────────────────────────────────────────────────

#[tokio::test]
async fn delete_account() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "name": "temp",
        "kind": "current",
        "icon": "bi-bank",
        "start_date": "2024-01-01",
        "start_balance_cents": 0
    }))
    .unwrap();
    let (_, body_str) = send(&app, authed_post("/accounts", &session, &csrf, body)).await;
    let created: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let id = created["id"].as_str().unwrap().to_string();

    let (status, _) = send(
        &app,
        authed_delete(&format!("/accounts/{id}"), &session, &csrf),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Verify gone
    let (status, _) = send(&app, authed_get(&format!("/accounts/{id}"), &session)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_referenced_account_blocked() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    // Create a current account (will be the primary)
    let body = serde_json::to_vec(&serde_json::json!({
        "name": "primary",
        "kind": "current",
        "icon": "bi-bank",
        "start_date": "2024-01-01",
        "start_balance_cents": 0
    }))
    .unwrap();
    let (_, body_str) = send(&app, authed_post("/accounts", &session, &csrf, body)).await;
    let primary: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let primary_id = primary["id"].as_str().unwrap().to_string();

    // Create a deposit that references the primary
    let body = serde_json::to_vec(&serde_json::json!({
        "name": "deposito",
        "kind": "deposit",
        "icon": "bi-piggy-bank",
        "start_date": "2024-01-01",
        "start_balance_cents": 100000,
        "rate_pct_bps": 300,
        "primary_account_id": primary_id
    }))
    .unwrap();
    let (status, body_str) = send(&app, authed_post("/accounts", &session, &csrf, body)).await;
    assert_eq!(status, StatusCode::CREATED, "body: {body_str}");

    // Attempt to delete the primary — should be blocked
    let (status, body_str) = send(
        &app,
        authed_delete(&format!("/accounts/{primary_id}"), &session, &csrf),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "body: {body_str}");
}

#[tokio::test]
async fn delete_missing_returns_404() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let (status, _) = send(&app, authed_delete("/accounts/ghost", &session, &csrf)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ─── multi-user gate tests ─────────────────────────────────────────────────────

const OTHER_USER: &str = "otheruser";
const ADMIN_USER: &str = "adminuser";

/// Build a test router with three users: owner (u1), other (u2), admin (u3).
async fn build_test_router_multi() -> (Router, Arc<AppState>) {
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

    // u1: owner (regular user)
    sqlx::query(
        "INSERT INTO users (id, username, email, password_hash, role, active, locale, default_currency, created_at)
         VALUES ('u1', $1, 'test@example.com', $2, 'user', 1, 'en', 'USD', '2025-01-01T00:00:00Z')",
    )
    .bind(TEST_USER)
    .bind(&hash)
    .execute(pool)
    .await
    .unwrap();

    // u2: other user (regular, no shares)
    sqlx::query(
        "INSERT INTO users (id, username, email, password_hash, role, active, locale, default_currency, created_at)
         VALUES ('u2', $1, 'other@example.com', $2, 'user', 1, 'en', 'USD', '2025-01-01T00:00:00Z')",
    )
    .bind(OTHER_USER)
    .bind(&hash)
    .execute(pool)
    .await
    .unwrap();

    // u3: admin
    sqlx::query(
        "INSERT INTO users (id, username, email, password_hash, role, active, locale, default_currency, created_at)
         VALUES ('u3', $1, 'admin@example.com', $2, 'admin', 1, 'en', 'USD', '2025-01-01T00:00:00Z')",
    )
    .bind(ADMIN_USER)
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
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            csrf_middleware,
        ))
        .layer(session_layer)
        .with_state(state.clone());

    (app, state)
}

/// Login as a specific user. Returns (session_cookie, csrf_token).
async fn login_as(app: &Router, username: &str) -> (String, String) {
    let body = serde_json::to_vec(&serde_json::json!({
        "username": username,
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
    assert_eq!(
        status,
        StatusCode::OK,
        "login as {username} failed: {body_str}"
    );

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

/// Gate: unshared user gets 403 on read of someone else's account.
#[tokio::test]
async fn unshared_user_cannot_read_others_account() {
    let (app, _) = build_test_router_multi().await;
    let (owner_session, owner_csrf) = login_as(&app, TEST_USER).await;

    // owner creates an account
    let body = serde_json::to_vec(&serde_json::json!({
        "name": "private account",
        "kind": "current",
        "icon": "bi-bank",
        "start_date": "2024-01-01",
        "start_balance_cents": 0
    }))
    .unwrap();
    let (status, body_str) = send(
        &app,
        authed_post("/accounts", &owner_session, &owner_csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body_str}");
    let created: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let id = created["id"].as_str().unwrap().to_string();

    // unshared user tries to read it
    let (other_session, _) = login_as(&app, OTHER_USER).await;
    let (status, _) = send(&app, authed_get(&format!("/accounts/{id}"), &other_session)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// Gate: unshared user gets 403 on write (PUT) of someone else's account.
#[tokio::test]
async fn unshared_user_cannot_write_others_account() {
    let (app, _) = build_test_router_multi().await;
    let (owner_session, owner_csrf) = login_as(&app, TEST_USER).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "name": "private account",
        "kind": "current",
        "icon": "bi-bank",
        "start_date": "2024-01-01",
        "start_balance_cents": 0
    }))
    .unwrap();
    let (_, body_str) = send(
        &app,
        authed_post("/accounts", &owner_session, &owner_csrf, body),
    )
    .await;
    let created: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let id = created["id"].as_str().unwrap().to_string();

    let (other_session, other_csrf) = login_as(&app, OTHER_USER).await;
    let update_body = serde_json::to_vec(&serde_json::json!({ "name": "hacked" })).unwrap();
    let (status, _) = send(
        &app,
        authed_put(
            &format!("/accounts/{id}"),
            &other_session,
            &other_csrf,
            update_body,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// Gate: unshared user gets 403 on delete of someone else's account.
#[tokio::test]
async fn unshared_user_cannot_delete_others_account() {
    let (app, _) = build_test_router_multi().await;
    let (owner_session, owner_csrf) = login_as(&app, TEST_USER).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "name": "private account",
        "kind": "current",
        "icon": "bi-bank",
        "start_date": "2024-01-01",
        "start_balance_cents": 0
    }))
    .unwrap();
    let (_, body_str) = send(
        &app,
        authed_post("/accounts", &owner_session, &owner_csrf, body),
    )
    .await;
    let created: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let id = created["id"].as_str().unwrap().to_string();

    let (other_session, other_csrf) = login_as(&app, OTHER_USER).await;
    let (status, _) = send(
        &app,
        authed_delete(&format!("/accounts/{id}"), &other_session, &other_csrf),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// Gate: owner has full access without any account_shares rows.
#[tokio::test]
async fn owner_has_full_access_without_shares_row() {
    let (app, _) = build_test_router_multi().await;
    let (owner_session, owner_csrf) = login_as(&app, TEST_USER).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "name": "my account",
        "kind": "current",
        "icon": "bi-bank",
        "start_date": "2024-01-01",
        "start_balance_cents": 0
    }))
    .unwrap();
    let (_, body_str) = send(
        &app,
        authed_post("/accounts", &owner_session, &owner_csrf, body),
    )
    .await;
    let created: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let id = created["id"].as_str().unwrap().to_string();

    // owner can read
    let (status, _) = send(&app, authed_get(&format!("/accounts/{id}"), &owner_session)).await;
    assert_eq!(status, StatusCode::OK);

    // owner can write
    let update_body = serde_json::to_vec(&serde_json::json!({ "name": "renamed" })).unwrap();
    let (status, _) = send(
        &app,
        authed_put(
            &format!("/accounts/{id}"),
            &owner_session,
            &owner_csrf,
            update_body,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // owner can delete
    let (status, _) = send(
        &app,
        authed_delete(&format!("/accounts/{id}"), &owner_session, &owner_csrf),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

/// Gate: admin bypasses all permission checks.
#[tokio::test]
async fn admin_can_read_any_account() {
    let (app, _) = build_test_router_multi().await;
    let (owner_session, owner_csrf) = login_as(&app, TEST_USER).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "name": "owner account",
        "kind": "current",
        "icon": "bi-bank",
        "start_date": "2024-01-01",
        "start_balance_cents": 0
    }))
    .unwrap();
    let (_, body_str) = send(
        &app,
        authed_post("/accounts", &owner_session, &owner_csrf, body),
    )
    .await;
    let created: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let id = created["id"].as_str().unwrap().to_string();

    let (admin_session, _) = login_as(&app, ADMIN_USER).await;
    let (status, body_str) =
        send(&app, authed_get(&format!("/accounts/{id}"), &admin_session)).await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
}

/// Gate: admin can write any account.
#[tokio::test]
async fn admin_can_write_any_account() {
    let (app, _) = build_test_router_multi().await;
    let (owner_session, owner_csrf) = login_as(&app, TEST_USER).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "name": "owner account",
        "kind": "current",
        "icon": "bi-bank",
        "start_date": "2024-01-01",
        "start_balance_cents": 0
    }))
    .unwrap();
    let (_, body_str) = send(
        &app,
        authed_post("/accounts", &owner_session, &owner_csrf, body),
    )
    .await;
    let created: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let id = created["id"].as_str().unwrap().to_string();

    let (admin_session, admin_csrf) = login_as(&app, ADMIN_USER).await;
    let update_body = serde_json::to_vec(&serde_json::json!({ "name": "admin-renamed" })).unwrap();
    let (status, body_str) = send(
        &app,
        authed_put(
            &format!("/accounts/{id}"),
            &admin_session,
            &admin_csrf,
            update_body,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
}

/// Gate: email search returns identical response for unregistered and
/// registered-but-unshared emails (non-leak property).
#[tokio::test]
async fn email_search_response_identical_for_unregistered_and_unshared() {
    let (app, _) = build_test_router_multi().await;
    let (owner_session, owner_csrf) = login_as(&app, TEST_USER).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "name": "shared account",
        "kind": "current",
        "icon": "bi-bank",
        "start_date": "2024-01-01",
        "start_balance_cents": 0
    }))
    .unwrap();
    let (_, body_str) = send(
        &app,
        authed_post("/accounts", &owner_session, &owner_csrf, body),
    )
    .await;
    let created: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let id = created["id"].as_str().unwrap().to_string();

    // Search for an unregistered email
    let (status1, body1) = send(
        &app,
        authed_get(
            &format!("/accounts/{id}/shares/search?email=ghost%40nonexistent.com"),
            &owner_session,
        ),
    )
    .await;
    assert_eq!(status1, StatusCode::OK);

    // Search for a registered but unshared email (other@example.com)
    let (status2, body2) = send(
        &app,
        authed_get(
            &format!("/accounts/{id}/shares/search?email=other%40example.com"),
            &owner_session,
        ),
    )
    .await;
    assert_eq!(status2, StatusCode::OK);

    // Both must return {"shared": false}
    assert_eq!(
        body1, body2,
        "responses must be identical to prevent info leakage"
    );
    let parsed: serde_json::Value = serde_json::from_str(&body1).unwrap();
    assert_eq!(parsed["shared"], false);
}

/// Gate: email search returns true for a registered user who IS shared.
#[tokio::test]
async fn email_search_returns_true_for_shared_user() {
    let (app, _) = build_test_router_multi().await;
    let (owner_session, owner_csrf) = login_as(&app, TEST_USER).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "name": "shared account",
        "kind": "current",
        "icon": "bi-bank",
        "start_date": "2024-01-01",
        "start_balance_cents": 0
    }))
    .unwrap();
    let (_, body_str) = send(
        &app,
        authed_post("/accounts", &owner_session, &owner_csrf, body),
    )
    .await;
    let created: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let id = created["id"].as_str().unwrap().to_string();

    // Owner grants read access to other@example.com
    let share_body = serde_json::to_vec(&serde_json::json!({
        "email": "other@example.com",
        "level": "read"
    }))
    .unwrap();
    let (status, body_str) = send(
        &app,
        authed_post(
            &format!("/accounts/{id}/shares"),
            &owner_session,
            &owner_csrf,
            share_body,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    // Now search for the shared email — should return true
    let (status, body) = send(
        &app,
        authed_get(
            &format!("/accounts/{id}/shares/search?email=other%40example.com"),
            &owner_session,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["shared"], true);
}
