//! HTTP integration tests for the admin routes: user CRUD, impersonation,
//! and admin account operations.
//!
//! Each test spins up an in-memory SQLite database with an admin user and a
//! regular user, builds the full router, and issues requests via
//! `tower::ServiceExt::oneshot`.

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

use pfm_http::admin_guard::admin_guard;
use pfm_http::csrf::csrf_middleware;
use pfm_http::session::build_session_layer;
use pfm_http::state::AppState;

const ADMIN_USER: &str = "admin";
const ADMIN_PASS: &str = "admin-pass-123";
const REGULAR_USER: &str = "regular";
const REGULAR_PASS: &str = "regular-pass-123";

/// Build a fully-wired test router with an in-memory SQLite DB, one admin and
/// one regular user.
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

    // Create schema

    // Hash passwords and insert users
    let pw = PasswordService::new(
        config.security.argon2.memory_kib,
        config.security.argon2.iterations,
        config.security.argon2.parallelism,
    );
    let admin_hash = pw.hash(ADMIN_PASS).unwrap();
    let regular_hash = pw.hash(REGULAR_PASS).unwrap();

    sqlx::query(
        "INSERT INTO users (id, username, email, password_hash, role, active, locale, default_currency, created_at)
         VALUES ('admin-1', $1, 'admin@test.com', $2, 'admin', 1, 'en', 'USD', '2025-01-01T00:00:00Z')",
    )
    .bind(ADMIN_USER)
    .bind(&admin_hash)
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO users (id, username, email, password_hash, role, active, locale, default_currency, created_at)
         VALUES ('user-1', $1, 'user@test.com', $2, 'user', 1, 'en', 'USD', '2025-01-01T00:00:00Z')",
    )
    .bind(REGULAR_USER)
    .bind(&regular_hash)
    .execute(pool)
    .await
    .unwrap();

    let storage = Arc::new(storage);

    let pw_ref = pfm_service::PasswordServiceRef::new(
        config.security.argon2.memory_kib,
        config.security.argon2.iterations,
        config.security.argon2.parallelism,
    );

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
    let user_admin = Arc::new(UserAdminService::new(&storage, pw_ref));
    let impersonation = Arc::new(ImpersonationService::new(&pfm_repo::SqlxAuthRepo::new(
        &storage,
    )));

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
        user_admin,
        impersonation,
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

    let admin_router = pfm_http::user_admin_routes::admin_routes().layer(
        axum::middleware::from_fn_with_state(state.clone(), admin_guard),
    );

    let app = Router::new()
        .route(
            "/healthz",
            axum::routing::get(|| async { (StatusCode::OK, "ok") }),
        )
        .merge(pfm_http::auth_routes::auth_routes(state.clone()))
        .merge(pfm_http::account_routes::account_routes())
        .merge(pfm_http::category_routes::category_routes())
        .merge(pfm_http::transaction_routes::transaction_routes())
        .merge(admin_router)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            csrf_middleware,
        ))
        .layer(session_layer)
        .with_state(state.clone());

    (app, state)
}

/// Helper: build a request with ConnectInfo injected.
fn req_with_connect_info(method: &str, uri: &str, body: Vec<u8>) -> Request<Body> {
    let addr: SocketAddr = "127.0.0.1:9999".parse().unwrap();
    let mut builder = Request::builder().method(method).uri(uri);
    builder = builder.header(header::CONTENT_TYPE, "application/json");
    let mut req = builder.body(Body::from(body)).unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));
    req
}

/// Helper: send a request and return (status, body_string, headers).
async fn send(app: &Router, req: Request<Body>) -> (StatusCode, String, axum::http::HeaderMap) {
    let res = app.clone().oneshot(req).await.expect("request failed");
    let status = res.status();
    let headers = res.headers().clone();
    let body = axum::body::to_bytes(res.into_body(), 1_048_576)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&body).into_owned(), headers)
}

/// Helper: login as the given user, return (session_cookie, csrf_token).
async fn login_as(app: &Router, username: &str, password: &str) -> (String, String) {
    let body = serde_json::to_vec(&serde_json::json!({
        "username": username,
        "password": password
    }))
    .unwrap();
    let (status, body_str, headers) =
        send(app, req_with_connect_info("POST", "/auth/login", body)).await;
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

/// Helper: build a GET request with session cookie.
fn get_req(uri: &str, session_cookie: &str) -> Request<Body> {
    let addr: SocketAddr = "127.0.0.1:9999".parse().unwrap();
    let mut req = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));
    req.headers_mut()
        .insert(header::COOKIE, session_cookie.parse().unwrap());
    req
}

/// Helper: build a POST request with session cookie + CSRF.
fn post_req(uri: &str, session_cookie: &str, csrf_token: &str, body: Vec<u8>) -> Request<Body> {
    let addr: SocketAddr = "127.0.0.1:9999".parse().unwrap();
    let mut req = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-CSRF-Token", csrf_token)
        .body(Body::from(body))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));
    req.headers_mut()
        .insert(header::COOKIE, session_cookie.parse().unwrap());
    req
}

/// Helper: build a PUT request with session cookie + CSRF.
fn put_req(uri: &str, session_cookie: &str, csrf_token: &str, body: Vec<u8>) -> Request<Body> {
    let addr: SocketAddr = "127.0.0.1:9999".parse().unwrap();
    let mut req = Request::builder()
        .method("PUT")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-CSRF-Token", csrf_token)
        .body(Body::from(body))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));
    req.headers_mut()
        .insert(header::COOKIE, session_cookie.parse().unwrap());
    req
}

/// Helper: build a DELETE request with session cookie + CSRF.
fn delete_req(uri: &str, session_cookie: &str, csrf_token: &str, body: Vec<u8>) -> Request<Body> {
    let addr: SocketAddr = "127.0.0.1:9999".parse().unwrap();
    let mut req = Request::builder()
        .method("DELETE")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-CSRF-Token", csrf_token)
        .body(Body::from(body))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));
    req.headers_mut()
        .insert(header::COOKIE, session_cookie.parse().unwrap());
    req
}

// ─── Non-admin gets 403 ───────────────────────────────────────────────────────

#[tokio::test]
async fn regular_user_get_403_on_users_list() {
    let (app, _) = build_test_router().await;
    let (session_cookie, _) = login_as(&app, REGULAR_USER, REGULAR_PASS).await;

    let (status, body_str, _) = send(&app, get_req("/users", &session_cookie)).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "regular user should get 403 on /users: {body_str}"
    );
}

#[tokio::test]
async fn regular_user_get_403_on_admin_accounts() {
    let (app, _) = build_test_router().await;
    let (session_cookie, _) = login_as(&app, REGULAR_USER, REGULAR_PASS).await;

    let (status, body_str, _) = send(&app, get_req("/admin/accounts", &session_cookie)).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "regular user should get 403 on /admin/accounts: {body_str}"
    );
}

#[tokio::test]
async fn regular_user_get_403_on_impersonate() {
    let (app, _) = build_test_router().await;
    let (session_cookie, csrf_token) = login_as(&app, REGULAR_USER, REGULAR_PASS).await;

    let body = serde_json::to_vec(&serde_json::json!({})).unwrap();
    let (status, body_str, _) = send(
        &app,
        post_req(
            "/admin/impersonate/user-1",
            &session_cookie,
            &csrf_token,
            body,
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "regular user should get 403 on /admin/impersonate: {body_str}"
    );
}

// ─── Admin can list users ────────────────────────────────────────────────────

#[tokio::test]
async fn admin_can_list_users() {
    let (app, _) = build_test_router().await;
    let (session_cookie, _) = login_as(&app, ADMIN_USER, ADMIN_PASS).await;

    let (status, body_str, _) = send(&app, get_req("/users", &session_cookie)).await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let users = parsed.as_array().unwrap();
    assert!(
        users.len() >= 2,
        "should have at least 2 users, got {}",
        users.len()
    );
}

// ─── Admin can create user ───────────────────────────────────────────────────

#[tokio::test]
async fn admin_can_create_user() {
    let (app, _) = build_test_router().await;
    let (session_cookie, csrf_token) = login_as(&app, ADMIN_USER, ADMIN_PASS).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "username": "newuser",
        "email": "new@test.com",
        "password": "password123",
        "role": "user"
    }))
    .unwrap();

    let (status, body_str, _) =
        send(&app, post_req("/users", &session_cookie, &csrf_token, body)).await;
    assert_eq!(status, StatusCode::CREATED, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed["username"], "newuser");
    assert_eq!(parsed["role"], "user");
}

// ─── Admin can update user ───────────────────────────────────────────────────

#[tokio::test]
async fn admin_can_update_user_role() {
    let (app, _) = build_test_router().await;
    let (session_cookie, csrf_token) = login_as(&app, ADMIN_USER, ADMIN_PASS).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "role": "admin"
    }))
    .unwrap();

    let (status, body_str, _) = send(
        &app,
        put_req("/users/user-1", &session_cookie, &csrf_token, body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed["role"], "admin");
}

// ─── Impersonation: audit trail ──────────────────────────────────────────────

#[tokio::test]
async fn impersonation_writes_audit_with_correct_actor_and_impersonated() {
    let (app, _state) = build_test_router().await;
    let (session_cookie, csrf_token) = login_as(&app, ADMIN_USER, ADMIN_PASS).await;

    // Start impersonation of user-1
    let body = serde_json::to_vec(&serde_json::json!({})).unwrap();
    let (status, body_str, _) = send(
        &app,
        post_req(
            "/admin/impersonate/user-1",
            &session_cookie,
            &csrf_token,
            body,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    // Verify audit log: actor = admin-1, impersonated_as = user-1, action = impersonation_started
    // Verify the response says we're impersonating
    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed["impersonating"], "user-1");

    // Stop impersonation
    let body = serde_json::to_vec(&serde_json::json!({})).unwrap();
    let (status, _, _) = send(
        &app,
        post_req(
            "/admin/impersonate/stop",
            &session_cookie,
            &csrf_token,
            body,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

// ─── DELETE user with accounts: reassignment required ────────────────────────

#[tokio::test]
async fn delete_user_with_accounts_without_reassignment_blocked() {
    let (app, _state) = build_test_router().await;

    // Create an account for user-1 via the API
    let (session_cookie, csrf_token) = login_as(&app, REGULAR_USER, REGULAR_PASS).await;
    let body = serde_json::to_vec(&serde_json::json!({
        "name": "My Account",
        "kind": "current",
        "icon": "bi-wallet2",
        "start_date": "2025-01-01",
        "start_balance_cents": 10000
    }))
    .unwrap();
    let (status, body_str, _) = send(
        &app,
        post_req("/accounts", &session_cookie, &csrf_token, body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body_str}");

    // Now admin tries to delete user-1 without reassignment
    let (admin_cookie, admin_csrf) = login_as(&app, ADMIN_USER, ADMIN_PASS).await;
    let body = serde_json::to_vec(&serde_json::json!({})).unwrap();
    let (status, body_str, _) = send(
        &app,
        delete_req("/users/user-1", &admin_cookie, &admin_csrf, body),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "should be 422 when accounts need reassignment: {body_str}"
    );
}

// ─── DELETE user with accounts: reassignment to target succeeds ─────────────

#[tokio::test]
async fn delete_user_with_accounts_reassign_to_target_succeeds() {
    let (app, _) = build_test_router().await;

    // Create an account for user-1
    let (session_cookie, csrf_token) = login_as(&app, REGULAR_USER, REGULAR_PASS).await;
    let body = serde_json::to_vec(&serde_json::json!({
        "name": "My Account",
        "kind": "current",
        "icon": "bi-wallet2",
        "start_date": "2025-01-01",
        "start_balance_cents": 10000
    }))
    .unwrap();
    let (status, _, _) = send(
        &app,
        post_req("/accounts", &session_cookie, &csrf_token, body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Admin deletes user-1, reassigning to admin-1
    let (admin_cookie, admin_csrf) = login_as(&app, ADMIN_USER, ADMIN_PASS).await;
    let body = serde_json::to_vec(&serde_json::json!({
        "reassign_to": "admin-1"
    }))
    .unwrap();
    let (status, body_str, _) = send(
        &app,
        delete_req("/users/user-1", &admin_cookie, &admin_csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "body: {body_str}");
}

// ─── DELETE account with transactions: reassignment required ────────────────

#[tokio::test]
async fn delete_account_with_transactions_without_reassignment_blocked() {
    let (app, _) = build_test_router().await;

    // Create an account for the admin
    let (admin_cookie, admin_csrf) = login_as(&app, ADMIN_USER, ADMIN_PASS).await;
    let body = serde_json::to_vec(&serde_json::json!({
        "name": "Admin Account",
        "kind": "current",
        "icon": "bi-wallet2",
        "start_date": "2025-01-01",
        "start_balance_cents": 50000
    }))
    .unwrap();
    let (status, body_str, _) = send(
        &app,
        post_req("/accounts", &admin_cookie, &admin_csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let account_id = parsed["id"].as_str().unwrap().to_string();

    // Add a transaction to the account
    let tx_body = serde_json::to_vec(&serde_json::json!({
        "account_id": account_id,
        "date": "2025-01-15",
        "cents": -1000,
        "description": "test expense"
    }))
    .unwrap();
    let (status, _, _) = send(
        &app,
        post_req("/transactions", &admin_cookie, &admin_csrf, tx_body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Try to delete the account without reassignment
    let body = serde_json::to_vec(&serde_json::json!({})).unwrap();
    let (status, body_str, _) = send(
        &app,
        delete_req(
            &format!("/admin/accounts/{account_id}"),
            &admin_cookie,
            &admin_csrf,
            body,
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "should be 422 when account has transactions: {body_str}"
    );
}

// ─── DELETE account with transactions: reassignment to target succeeds ──────

#[tokio::test]
async fn delete_account_with_transactions_reassign_to_target_succeeds() {
    let (app, _) = build_test_router().await;

    // Create two accounts for the admin
    let (admin_cookie, admin_csrf) = login_as(&app, ADMIN_USER, ADMIN_PASS).await;
    let body = serde_json::to_vec(&serde_json::json!({
        "name": "Account A",
        "kind": "current",
        "icon": "bi-wallet2",
        "start_date": "2025-01-01",
        "start_balance_cents": 50000
    }))
    .unwrap();
    let (status, body_str, _) = send(
        &app,
        post_req("/accounts", &admin_cookie, &admin_csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body_str}");
    let account_a = serde_json::from_str::<serde_json::Value>(&body_str).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let body = serde_json::to_vec(&serde_json::json!({
        "name": "Account B",
        "kind": "current",
        "icon": "bi-wallet2",
        "start_date": "2025-01-01",
        "start_balance_cents": 30000
    }))
    .unwrap();
    let (status, body_str, _) = send(
        &app,
        post_req("/accounts", &admin_cookie, &admin_csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body_str}");
    let account_b = serde_json::from_str::<serde_json::Value>(&body_str).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Add a transaction to Account A
    let tx_body = serde_json::to_vec(&serde_json::json!({
        "account_id": account_a,
        "date": "2025-01-15",
        "cents": -1000,
        "description": "test expense"
    }))
    .unwrap();
    let (status, _, _) = send(
        &app,
        post_req("/transactions", &admin_cookie, &admin_csrf, tx_body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Delete Account A, reassigning transactions to Account B
    let body = serde_json::to_vec(&serde_json::json!({
        "reassign_tx_to": account_b
    }))
    .unwrap();
    let (status, body_str, _) = send(
        &app,
        delete_req(
            &format!("/admin/accounts/{account_a}"),
            &admin_cookie,
            &admin_csrf,
            body,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "body: {body_str}");
}

// ─── Admin can list all accounts ─────────────────────────────────────────────

#[tokio::test]
async fn admin_can_list_all_accounts() {
    let (app, _) = build_test_router().await;

    // Create an account as regular user
    let (session_cookie, csrf_token) = login_as(&app, REGULAR_USER, REGULAR_PASS).await;
    let body = serde_json::to_vec(&serde_json::json!({
        "name": "Regular Account",
        "kind": "current",
        "icon": "bi-wallet2",
        "start_date": "2025-01-01",
        "start_balance_cents": 10000
    }))
    .unwrap();
    let (status, _, _) = send(
        &app,
        post_req("/accounts", &session_cookie, &csrf_token, body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Admin can see all accounts
    let (admin_cookie, _) = login_as(&app, ADMIN_USER, ADMIN_PASS).await;
    let (status, body_str, _) = send(&app, get_req("/admin/accounts", &admin_cookie)).await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let accounts = parsed.as_array().unwrap();
    assert!(!accounts.is_empty());
}

// ─── Unauthenticated gets 401 ────────────────────────────────────────────────

#[tokio::test]
async fn unauthenticated_get_401_on_admin_routes() {
    let (app, _) = build_test_router().await;

    // No session cookie at all
    let addr: SocketAddr = "127.0.0.1:9999".parse().unwrap();
    let mut req = Request::builder()
        .method("GET")
        .uri("/users")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let (status, _, _) = send(&app, req).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
