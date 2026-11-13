//! HTTP integration tests for the transactions routes.

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
const OTHER_USER: &str = "otheruser";
const TEST_PASS: &str = "correct-horse-battery-staple";

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

    // Real schema, from the migrations the application actually runs — indexes
    // included. The hand-written DDL this replaces defined no indexes at all,
    // so constraint violations that break production went unnoticed here.
    let migrator = sqlx::migrate!("../../migrations");
    storage.migrate(&migrator).await.expect("migrations");

    let pool = storage.pool();
    let pool_owned = pool.clone();

    // Unique index on client_id for idempotent offline sync.
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_transactions_client_id ON transactions (client_id)",
    )
    .execute(pool)
    .await
    .unwrap();

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

    // u2: second user (regular, no shares) for permission tests
    sqlx::query(
        "INSERT INTO users (id, username, email, password_hash, role, active, locale, default_currency, created_at)
         VALUES ('u2', $1, 'other@example.com', $2, 'user', 1, 'en', 'USD', '2025-01-01T00:00:00Z')",
    )
    .bind(OTHER_USER)
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
        .merge(pfm_http::tag_routes::tag_routes())
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

/// Create an account via the API and return its ID.
async fn create_account(app: &Router, session: &str, csrf: &str) -> String {
    let body = serde_json::to_vec(&serde_json::json!({
        "name": "test account",
        "kind": "current",
        "icon": "bi-wallet",
        "start_date": "2024-01-01",
        "start_balance_cents": 100000
    }))
    .unwrap();
    let (status, body_str) = send(app, authed_post("/accounts", session, csrf, body)).await;
    assert_eq!(status, StatusCode::CREATED, "create account: {body_str}");
    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    parsed["id"].as_str().unwrap().to_string()
}

// â”€â”€â”€ transactions: requires auth â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[tokio::test]
async fn transactions_requires_session() {
    let (app, _, _) = build_test_router().await;
    let (status, _) = send(
        &app,
        authed_get(
            "/transactions?from=2024-01-01&to=2024-01-31&account=acc1",
            "",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// â”€â”€â”€ transactions: create â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[tokio::test]
async fn create_transaction() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let account_id = create_account(&app, &session, &csrf).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "account_id": account_id,
        "date": "2024-06-15",
        "cents": -5000,
        "description": "groceries",
        "category_id": "cat-fallback-out",
        "note": "weekly shop"
    }))
    .unwrap();

    let (status, body_str) = send(&app, authed_post("/transactions", &session, &csrf, body)).await;
    assert_eq!(status, StatusCode::CREATED, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert!(!parsed["id"].as_str().unwrap().is_empty());
    assert_eq!(parsed["date"], "2024-06-15");
    assert_eq!(parsed["cents"], -5000);
    assert_eq!(parsed["description"], "groceries");
    assert_eq!(parsed["category_id"], "cat-fallback-out");
    assert_eq!(parsed["note"], "weekly shop");
}

#[tokio::test]
async fn create_transaction_invalid_date() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let account_id = create_account(&app, &session, &csrf).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "account_id": account_id,
        "date": "not-a-date",
        "cents": 100
    }))
    .unwrap();

    let (status, _) = send(&app, authed_post("/transactions", &session, &csrf, body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_transaction_nonexistent_account() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "account_id": "nonexistent-account",
        "date": "2024-06-15",
        "cents": -5000
    }))
    .unwrap();

    let (status, _) = send(&app, authed_post("/transactions", &session, &csrf, body)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// â”€â”€â”€ transactions: get â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[tokio::test]
async fn get_transaction() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let account_id = create_account(&app, &session, &csrf).await;

    let create_body = serde_json::to_vec(&serde_json::json!({
        "account_id": account_id,
        "date": "2024-06-15",
        "cents": -5000,
        "description": "test"
    }))
    .unwrap();
    let (_, create_str) = send(
        &app,
        authed_post("/transactions", &session, &csrf, create_body),
    )
    .await;
    let created: serde_json::Value = serde_json::from_str(&create_str).unwrap();
    let tx_id = created["id"].as_str().unwrap().to_string();

    let (status, body_str) = send(
        &app,
        authed_get(&format!("/transactions/{tx_id}"), &session),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed["id"], tx_id);
    assert_eq!(parsed["description"], "test");
}

#[tokio::test]
async fn get_transaction_not_found() {
    let (app, _, _) = build_test_router().await;
    let (session, _) = login(&app).await;

    let (status, _) = send(&app, authed_get("/transactions/nonexistent-id", &session)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// â”€â”€â”€ transactions: update â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[tokio::test]
async fn update_transaction() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let account_id = create_account(&app, &session, &csrf).await;

    let create_body = serde_json::to_vec(&serde_json::json!({
        "account_id": account_id,
        "date": "2024-06-15",
        "cents": -5000,
        "description": "original"
    }))
    .unwrap();
    let (_, create_str) = send(
        &app,
        authed_post("/transactions", &session, &csrf, create_body),
    )
    .await;
    let created: serde_json::Value = serde_json::from_str(&create_str).unwrap();
    let tx_id = created["id"].as_str().unwrap().to_string();

    let update_body = serde_json::to_vec(&serde_json::json!({
        "description": "updated",
        "cents": -3000
    }))
    .unwrap();
    let (status, body_str) = send(
        &app,
        authed_put(
            &format!("/transactions/{tx_id}"),
            &session,
            &csrf,
            update_body,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed["description"], "updated");
    assert_eq!(parsed["cents"], -3000);
}

/// Create a plain movement on `account_id` and return its id.
async fn create_tx_on(app: &Router, session: &str, csrf: &str, account_id: &str) -> String {
    let body = serde_json::to_vec(&serde_json::json!({
        "account_id": account_id, "date": "2024-06-15", "cents": -5000, "description": "move me"
    }))
    .unwrap();
    let (status, s) = send(app, authed_post("/transactions", session, csrf, body)).await;
    assert_eq!(status, StatusCode::CREATED, "body: {s}");
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    v["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn update_transaction_changes_account() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let from = create_account(&app, &session, &csrf).await;
    let to = create_account(&app, &session, &csrf).await;
    let tx_id = create_tx_on(&app, &session, &csrf, &from).await;

    let body = serde_json::to_vec(&serde_json::json!({ "account_id": to })).unwrap();
    let (status, body_str) = send(
        &app,
        authed_put(&format!("/transactions/{tx_id}"), &session, &csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed["account_id"], to.as_str());

    // The change is persisted, not just echoed.
    let (_, got) = send(
        &app,
        authed_get(&format!("/transactions/{tx_id}"), &session),
    )
    .await;
    let got: serde_json::Value = serde_json::from_str(&got).unwrap();
    assert_eq!(got["account_id"], to.as_str());

    // Sending the current account again is a harmless no-op.
    let body = serde_json::to_vec(&serde_json::json!({ "account_id": to, "note": "x" })).unwrap();
    let (status, _) = send(
        &app,
        authed_put(&format!("/transactions/{tx_id}"), &session, &csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn update_transaction_account_unknown_or_foreign() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login_as(&app, TEST_USER).await;
    let (other_session, other_csrf) = login_as(&app, OTHER_USER).await;
    let from = create_account(&app, &session, &csrf).await;
    let foreign = create_account(&app, &other_session, &other_csrf).await;
    let tx_id = create_tx_on(&app, &session, &csrf, &from).await;

    let put = |dest: &str| {
        authed_put(
            &format!("/transactions/{tx_id}"),
            &session,
            &csrf,
            serde_json::to_vec(&serde_json::json!({ "account_id": dest })).unwrap(),
        )
    };
    let (status, b) = send(&app, put("no-such-account")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body: {b}");
    let (status, b) = send(&app, put(&foreign)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {b}");

    // Neither attempt moved the row.
    let (_, got) = send(
        &app,
        authed_get(&format!("/transactions/{tx_id}"), &session),
    )
    .await;
    let got: serde_json::Value = serde_json::from_str(&got).unwrap();
    assert_eq!(got["account_id"], from.as_str());
}

#[tokio::test]
async fn update_transaction_account_locked_for_special_rows() {
    let (app, _, pool) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let from = create_account(&app, &session, &csrf).await;
    let to = create_account(&app, &session, &csrf).await;

    for (column, sql) in [
        (
            "pair_id",
            "UPDATE transactions SET pair_id = 'pair-1' WHERE id = $1",
        ),
        (
            "import_hash",
            "UPDATE transactions SET import_hash = 'hash-1' WHERE id = $1",
        ),
    ] {
        let tx_id = create_tx_on(&app, &session, &csrf, &from).await;
        sqlx::query(sql).bind(&tx_id).execute(&pool).await.unwrap();

        // Moving it is refused...
        let body = serde_json::to_vec(&serde_json::json!({ "account_id": to })).unwrap();
        let (status, b) = send(
            &app,
            authed_put(&format!("/transactions/{tx_id}"), &session, &csrf, body),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{column}: {b}");

        // ...but the other fields can still be edited (same account).
        let body =
            serde_json::to_vec(&serde_json::json!({ "account_id": from, "description": "edited" }))
                .unwrap();
        let (status, b) = send(
            &app,
            authed_put(&format!("/transactions/{tx_id}"), &session, &csrf, body),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{column}: {b}");

        let (_, got) = send(
            &app,
            authed_get(&format!("/transactions/{tx_id}"), &session),
        )
        .await;
        let got: serde_json::Value = serde_json::from_str(&got).unwrap();
        assert_eq!(got["account_id"], from.as_str(), "{column}");
    }
}

#[tokio::test]
async fn register_flags_rows_with_locked_account() {
    let (app, _, pool) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let acc = create_account(&app, &session, &csrf).await;
    let plain = create_tx_on(&app, &session, &csrf, &acc).await;
    let locked = create_tx_on(&app, &session, &csrf, &acc).await;
    sqlx::query("UPDATE transactions SET pair_id = 'pair-x' WHERE id = $1")
        .bind(&locked)
        .execute(&pool)
        .await
        .unwrap();

    let (status, b) = send(
        &app,
        authed_get(
            &format!("/transactions?from=2024-06-01&to=2024-06-30&account={acc}"),
            &session,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {b}");
    let v: serde_json::Value = serde_json::from_str(&b).unwrap();
    let mut seen = std::collections::HashMap::new();
    for day in v["days"].as_array().unwrap() {
        for row in day["rows"].as_array().unwrap() {
            seen.insert(
                row["id"].as_str().unwrap().to_string(),
                row["account_locked"].as_bool().unwrap_or(false),
            );
        }
    }
    assert_eq!(seen.get(&plain), Some(&false));
    assert_eq!(seen.get(&locked), Some(&true));
}

#[tokio::test]
async fn update_transaction_not_found() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "description": "new"
    }))
    .unwrap();
    let (status, _) = send(
        &app,
        authed_put("/transactions/nonexistent-id", &session, &csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// â”€â”€â”€ transactions: delete â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[tokio::test]
async fn delete_transaction() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let account_id = create_account(&app, &session, &csrf).await;

    let create_body = serde_json::to_vec(&serde_json::json!({
        "account_id": account_id,
        "date": "2024-06-15",
        "cents": -5000
    }))
    .unwrap();
    let (_, create_str) = send(
        &app,
        authed_post("/transactions", &session, &csrf, create_body),
    )
    .await;
    let created: serde_json::Value = serde_json::from_str(&create_str).unwrap();
    let tx_id = created["id"].as_str().unwrap().to_string();

    let (status, _) = send(
        &app,
        authed_delete(&format!("/transactions/{tx_id}"), &session, &csrf),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Verify it's gone
    let (status, _) = send(
        &app,
        authed_get(&format!("/transactions/{tx_id}"), &session),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_transaction_not_found() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let (status, _) = send(
        &app,
        authed_delete("/transactions/nonexistent-id", &session, &csrf),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// â”€â”€â”€ register: basic â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[tokio::test]
async fn register_basic() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let account_id = create_account(&app, &session, &csrf).await;

    // Create a few transactions on different days
    let tx1 = serde_json::to_vec(&serde_json::json!({
        "account_id": account_id, "date": "2024-06-01", "cents": 100000
    }))
    .unwrap();
    let (status, _) = send(&app, authed_post("/transactions", &session, &csrf, tx1)).await;
    assert_eq!(status, StatusCode::CREATED);

    let tx2 = serde_json::to_vec(&serde_json::json!({
        "account_id": account_id, "date": "2024-06-03", "cents": -50000
    }))
    .unwrap();
    let (status, _) = send(&app, authed_post("/transactions", &session, &csrf, tx2)).await;
    assert_eq!(status, StatusCode::CREATED);

    let tx3 = serde_json::to_vec(&serde_json::json!({
        "account_id": account_id, "date": "2024-06-03", "cents": -25000
    }))
    .unwrap();
    let (status, _) = send(&app, authed_post("/transactions", &session, &csrf, tx3)).await;
    assert_eq!(status, StatusCode::CREATED);

    // Query the register for June 1-5
    let (status, body_str) = send(
        &app,
        authed_get(
            &format!("/transactions?from=2024-06-01&to=2024-06-05&account={account_id}"),
            &session,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let days = parsed["days"].as_array().unwrap();
    // 5 days: Jun 1-5
    assert_eq!(days.len(), 5);

    // Day 1 (Jun 1): has 1 row, not empty
    assert_eq!(days[0]["date"], "2024-06-01");
    assert!(!days[0]["is_empty"].as_bool().unwrap());
    assert_eq!(days[0]["rows"].as_array().unwrap().len(), 1);
    // Closing balance: opening (100000) + 100000 = 200000
    assert_eq!(days[0]["closing_balance_cents"], 200000);

    // Day 2 (Jun 2): empty â€” closing balance carried forward from Jun 1
    assert_eq!(days[1]["date"], "2024-06-02");
    assert!(days[1]["is_empty"].as_bool().unwrap());
    assert_eq!(days[1]["rows"].as_array().unwrap().len(), 0);
    assert_eq!(days[1]["closing_balance_cents"], 200000);

    // Day 3 (Jun 3): has 2 rows
    assert_eq!(days[2]["date"], "2024-06-03");
    assert!(!days[2]["is_empty"].as_bool().unwrap());
    assert_eq!(days[2]["rows"].as_array().unwrap().len(), 2);
    // Closing: 200000 - 50000 - 25000 = 125000
    assert_eq!(days[2]["closing_balance_cents"], 125000);

    // Day 4 (Jun 4): empty â€” closing carried from Jun 3
    assert_eq!(days[3]["date"], "2024-06-04");
    assert!(days[3]["is_empty"].as_bool().unwrap());
    assert_eq!(days[3]["closing_balance_cents"], 125000);

    // Day 5 (Jun 5): empty â€” closing carried from Jun 3
    assert_eq!(days[4]["date"], "2024-06-05");
    assert!(days[4]["is_empty"].as_bool().unwrap());
    assert_eq!(days[4]["closing_balance_cents"], 125000);

    // Summary: income=100000, expense=75000, net=25000
    assert_eq!(parsed["summary"]["income_cents"], 100000);
    assert_eq!(parsed["summary"]["expense_cents"], 75000);
    assert_eq!(parsed["summary"]["net_cents"], 25000);
    assert_eq!(parsed["summary"]["opening_cents"], 100000);
    assert_eq!(parsed["summary"]["closing_cents"], 125000);
}

// â”€â”€â”€ register: missing required params â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[tokio::test]
async fn register_missing_from() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let account_id = create_account(&app, &session, &csrf).await;

    let (status, _) = send(
        &app,
        authed_get(
            &format!("/transactions?to=2024-06-30&account={account_id}"),
            &session,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn register_missing_account() {
    let (app, _, _) = build_test_router().await;
    let (session, _) = login(&app).await;

    let (status, body) = send(
        &app,
        authed_get("/transactions?from=2024-06-01&to=2024-06-30", &session),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // No account specified: empty register
    let resp: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(resp["days"].as_array().unwrap().len(), 0);
    assert_eq!(resp["summary"]["closing_cents"], 0);
}

#[tokio::test]
async fn register_from_after_to() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let account_id = create_account(&app, &session, &csrf).await;

    let (status, _) = send(
        &app,
        authed_get(
            &format!("/transactions?from=2024-06-30&to=2024-06-01&account={account_id}"),
            &session,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// â”€â”€â”€ register: filters â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[tokio::test]
async fn register_filter_by_type() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let account_id = create_account(&app, &session, &csrf).await;

    // Income
    let tx_in = serde_json::to_vec(&serde_json::json!({
        "account_id": account_id, "date": "2024-06-01", "cents": 50000, "description": "salary"
    }))
    .unwrap();
    send(&app, authed_post("/transactions", &session, &csrf, tx_in)).await;

    // Expense
    let tx_out = serde_json::to_vec(&serde_json::json!({
        "account_id": account_id, "date": "2024-06-02", "cents": -30000, "description": "rent"
    }))
    .unwrap();
    send(&app, authed_post("/transactions", &session, &csrf, tx_out)).await;

    // Filter by tipo=in
    let (status, body_str) = send(
        &app,
        authed_get(
            &format!("/transactions?from=2024-06-01&to=2024-06-30&account={account_id}&tipo=in"),
            &session,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let days = parsed["days"].as_array().unwrap();
    // Only Jun 1 has an income row; Jun 2 is empty (the expense is filtered out)
    assert!(!days[0]["is_empty"].as_bool().unwrap());
    assert!(days[1]["is_empty"].as_bool().unwrap());
    assert_eq!(parsed["summary"]["income_cents"], 50000);
    assert_eq!(parsed["summary"]["expense_cents"], 0);

    // Filter by tipo=out
    let (status, body_str) = send(
        &app,
        authed_get(
            &format!("/transactions?from=2024-06-01&to=2024-06-30&account={account_id}&tipo=out"),
            &session,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let days = parsed["days"].as_array().unwrap();
    assert!(days[0]["is_empty"].as_bool().unwrap());
    assert!(!days[1]["is_empty"].as_bool().unwrap());
    assert_eq!(parsed["summary"]["income_cents"], 0);
    assert_eq!(parsed["summary"]["expense_cents"], 30000);
}

#[tokio::test]
async fn register_filter_by_min_max() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let account_id = create_account(&app, &session, &csrf).await;

    // Small expense
    let tx1 = serde_json::to_vec(&serde_json::json!({
        "account_id": account_id, "date": "2024-06-01", "cents": -10000
    }))
    .unwrap();
    send(&app, authed_post("/transactions", &session, &csrf, tx1)).await;

    // Large expense
    let tx2 = serde_json::to_vec(&serde_json::json!({
        "account_id": account_id, "date": "2024-06-02", "cents": -100000
    }))
    .unwrap();
    send(&app, authed_post("/transactions", &session, &csrf, tx2)).await;

    // Filter min=50000 (only the large one passes; min applies to absolute value for expenses)
    let (status, body_str) = send(
        &app,
        authed_get(
            &format!("/transactions?from=2024-06-01&to=2024-06-30&account={account_id}&min=50000"),
            &session,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let days = parsed["days"].as_array().unwrap();
    // The small one (10000) is filtered out; the large one (100000) passes
    assert!(days[0]["is_empty"].as_bool().unwrap());
    assert!(!days[1]["is_empty"].as_bool().unwrap());
}

#[tokio::test]
async fn register_filter_by_text() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let account_id = create_account(&app, &session, &csrf).await;

    let tx1 = serde_json::to_vec(&serde_json::json!({
        "account_id": account_id, "date": "2024-06-01", "cents": -10000, "description": "supermarket"
    }))
    .unwrap();
    send(&app, authed_post("/transactions", &session, &csrf, tx1)).await;

    let tx2 = serde_json::to_vec(&serde_json::json!({
        "account_id": account_id, "date": "2024-06-02", "cents": -20000, "description": "restaurant"
    }))
    .unwrap();
    send(&app, authed_post("/transactions", &session, &csrf, tx2)).await;

    // Search for "super"
    let (status, body_str) = send(
        &app,
        authed_get(
            &format!("/transactions?from=2024-06-01&to=2024-06-30&account={account_id}&q=super"),
            &session,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let days = parsed["days"].as_array().unwrap();
    assert!(!days[0]["is_empty"].as_bool().unwrap());
    assert!(days[1]["is_empty"].as_bool().unwrap());
}

// â”€â”€â”€ register: nonexistent account â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[tokio::test]
async fn register_nonexistent_account() {
    let (app, _, _) = build_test_router().await;
    let (session, _) = login(&app).await;

    let (status, _) = send(
        &app,
        authed_get(
            "/transactions?from=2024-06-01&to=2024-06-30&account=nonexistent",
            &session,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// â”€â”€â”€ register: large dataset (>1000 rows) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[tokio::test]
async fn register_large_dataset() {
    let (app, _state, pool) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let account_id = create_account(&app, &session, &csrf).await;

    // Bulk-insert 1500 transactions directly via SQL for speed.
    // Pattern: 30 days x 50 tx/day = 1500 rows, spread over 30 days in June.

    let mut total_cents: i64 = 0;
    for i in 0..1500i64 {
        let day = (i / 50) % 30 + 1;
        let date = format!("2024-06-{day:02}");
        let cents = if i % 2 == 0 {
            1000 + (i % 100)
        } else {
            -(1000 + (i % 100))
        };
        total_cents += cents;

        sqlx::query(
            "INSERT INTO transactions (id, client_id, account_id, date, cents, description, created_by, created_at, updated_at)
             VALUES ($1, $6, $2, $3, $4, $5, 'u1', '2024-06-01T00:00:00Z', '2024-06-01T00:00:00Z')",
        )
        .bind(format!("tx-{i:04}"))
        .bind(&account_id)
        .bind(&date)
        .bind(cents)
        .bind(format!("tx {i}"))
        .bind(format!("bulk-{i:04}"))
        .execute(&pool)
        .await
        .unwrap();
    }

    // Query the register for all of June.
    let start = std::time::Instant::now();
    let (status, body_str) = send(
        &app,
        authed_get(
            &format!("/transactions?from=2024-06-01&to=2024-06-30&account={account_id}"),
            &session,
        ),
    )
    .await;
    let elapsed = start.elapsed();
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "register took too long: {elapsed:?}"
    );

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let days = parsed["days"].as_array().unwrap();
    assert_eq!(days.len(), 30);

    // No day should be empty (50 tx per day)
    for d in days {
        assert!(
            !d["is_empty"].as_bool().unwrap(),
            "day {} should not be empty",
            d["date"]
        );
    }

    // Verify closing balance: start_balance (100000) + total_cents
    let expected_closing = 100_000i64 + total_cents;
    assert_eq!(
        parsed["summary"]["closing_cents"], expected_closing,
        "closing balance mismatch"
    );
    assert_eq!(
        parsed["summary"]["opening_cents"], 100_000i64,
        "opening balance should be start_balance"
    );

    // Verify the last day's closing matches the summary closing
    let last_day = days.last().unwrap();
    assert_eq!(
        last_day["closing_balance_cents"], expected_closing,
        "last day closing should match summary"
    );
}

// â”€â”€â”€ register: combined filters â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[tokio::test]
async fn register_combined_filters() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let account_id = create_account(&app, &session, &csrf).await;

    // Create a custom category
    let cat_body = serde_json::to_vec(&serde_json::json!({
        "name": "Food", "emoji": "U+1F374", "kind": "out"
    }))
    .unwrap();
    let (status, cat_str) = send(&app, authed_post("/categories", &session, &csrf, cat_body)).await;
    assert_eq!(status, StatusCode::CREATED, "body: {cat_str}");
    let cat_parsed: serde_json::Value = serde_json::from_str(&cat_str).unwrap();
    let cat_id = cat_parsed["id"].as_str().unwrap().to_string();

    // Tx1: category=Food, type=out, cents=-100000, desc="supermarket" (should match all filters)
    let tx1 = serde_json::to_vec(&serde_json::json!({
        "account_id": account_id, "date": "2024-06-01", "cents": -100000,
        "description": "supermarket", "category_id": cat_id
    }))
    .unwrap();
    send(&app, authed_post("/transactions", &session, &csrf, tx1)).await;

    // Tx2: category=Food, type=out, cents=-20000, desc="restaurant" (fails min filter)
    let tx2 = serde_json::to_vec(&serde_json::json!({
        "account_id": account_id, "date": "2024-06-01", "cents": -20000,
        "description": "restaurant", "category_id": cat_id
    }))
    .unwrap();
    send(&app, authed_post("/transactions", &session, &csrf, tx2)).await;

    // Tx3: category=null, type=out, cents=-200000, desc="supermarket run" (fails category filter)
    let tx3 = serde_json::to_vec(&serde_json::json!({
        "account_id": account_id, "date": "2024-06-02", "cents": -200000,
        "description": "supermarket run"
    }))
    .unwrap();
    send(&app, authed_post("/transactions", &session, &csrf, tx3)).await;

    // Tx4: category=Food, type=in, cents=50000, desc="supermarket refund" (fails type filter)
    let tx4 = serde_json::to_vec(&serde_json::json!({
        "account_id": account_id, "date": "2024-06-02", "cents": 50000,
        "description": "supermarket refund", "category_id": cat_id
    }))
    .unwrap();
    send(&app, authed_post("/transactions", &session, &csrf, tx4)).await;

    // Combined: cat=Food + tipo=out + min=50000 + q=super
    // Only Tx1 matches all four: category=Food, negative, >=50000 abs, contains "super"
    let (status, body_str) = send(
        &app,
        authed_get(
            &format!("/transactions?from=2024-06-01&to=2024-06-03&account={account_id}&cat={cat_id}&tipo=out&min=50000&q=super"),
            &session,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let days = parsed["days"].as_array().unwrap();

    // Jun 1: only tx1 passes (tx2 fails min, tx3 fails cat)
    assert!(!days[0]["is_empty"].as_bool().unwrap());
    let rows_d0 = days[0]["rows"].as_array().unwrap();
    assert_eq!(rows_d0.len(), 1);
    assert_eq!(rows_d0[0]["description"], "supermarket");

    // Jun 2: tx3 fails cat, tx4 fails tipo â†’ empty
    assert!(days[1]["is_empty"].as_bool().unwrap());

    // Jun 3: no tx at all â†’ empty
    assert!(days[2]["is_empty"].as_bool().unwrap());

    // Summary: only tx1 counts
    assert_eq!(parsed["summary"]["income_cents"], 0);
    assert_eq!(parsed["summary"]["expense_cents"], 100000);

    // Now test without the category filter: tipo=out + min=50000 + q=super
    // Tx1 (cat=Food, -100000, "supermarket") and Tx3 (no cat, -200000, "supermarket run") pass
    let (status, body_str) = send(
        &app,
        authed_get(
            &format!("/transactions?from=2024-06-01&to=2024-06-03&account={account_id}&tipo=out&min=50000&q=super"),
            &session,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let days = parsed["days"].as_array().unwrap();

    // Jun 1: tx1 matches (tx2 fails min)
    assert!(!days[0]["is_empty"].as_bool().unwrap());
    assert_eq!(days[0]["rows"].as_array().unwrap().len(), 1);

    // Jun 2: tx3 matches (tx4 fails tipo)
    assert!(!days[1]["is_empty"].as_bool().unwrap());
    assert_eq!(days[1]["rows"].as_array().unwrap().len(), 1);
    assert_eq!(parsed["summary"]["expense_cents"], 300000);
}

// â”€â”€â”€ permissions: unauthorized user â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[tokio::test]
async fn unauthorized_user_cannot_read_transaction() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login_as(&app, TEST_USER).await;
    let (other_session, _) = login_as(&app, OTHER_USER).await;
    let account_id = create_account(&app, &session, &csrf).await;

    // Owner creates a transaction
    let body = serde_json::to_vec(&serde_json::json!({
        "account_id": account_id, "date": "2024-06-15", "cents": -5000, "description": "secret"
    }))
    .unwrap();
    let (status, create_str) =
        send(&app, authed_post("/transactions", &session, &csrf, body)).await;
    assert_eq!(status, StatusCode::CREATED, "body: {create_str}");
    let created: serde_json::Value = serde_json::from_str(&create_str).unwrap();
    let tx_id = created["id"].as_str().unwrap().to_string();

    // Unshared user tries to read it â†’ 403
    let (status, body_str) = send(
        &app,
        authed_get(&format!("/transactions/{tx_id}"), &other_session),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body_str}");
}

#[tokio::test]
async fn unauthorized_user_cannot_read_register() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login_as(&app, TEST_USER).await;
    let (other_session, _) = login_as(&app, OTHER_USER).await;
    let account_id = create_account(&app, &session, &csrf).await;

    // Unshared user tries to read the register for owner's account â†’ 403
    let (status, body_str) = send(
        &app,
        authed_get(
            &format!("/transactions?from=2024-06-01&to=2024-06-30&account={account_id}"),
            &other_session,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body_str}");
}

#[tokio::test]
async fn unauthorized_user_cannot_create_transaction() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login_as(&app, TEST_USER).await;
    let (other_session, other_csrf) = login_as(&app, OTHER_USER).await;
    let account_id = create_account(&app, &session, &csrf).await;

    // Unshared user tries to create a tx on owner's account â†’ 403
    let body = serde_json::to_vec(&serde_json::json!({
        "account_id": account_id, "date": "2024-06-15", "cents": -1000
    }))
    .unwrap();
    let (status, body_str) = send(
        &app,
        authed_post("/transactions", &other_session, &other_csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body_str}");
}

// â”€â”€â”€ transactions: idempotent double-send (offline sync) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// When the same client_id is sent twice (e.g. double-tap or network retry),
/// the server must create exactly one row and return the same payload both times.
#[tokio::test]
async fn double_send_same_client_id_is_idempotent() {
    let (app, _, pool) = build_test_router().await;
    let (session, csrf) = login_as(&app, TEST_USER).await;
    let account_id = create_account(&app, &session, &csrf).await;

    // A fixed client_id simulating what the PWA would generate offline.
    let client_id = "offline-uuid-1111-2222-3333-444455556666";

    let body = serde_json::to_vec(&serde_json::json!({
        "account_id": account_id,
        "date": "2024-07-01",
        "cents": -15000,
        "description": "offline purchase",
        "client_id": client_id
    }))
    .unwrap();

    // First send: creates the row.
    let (status1, body1) = send(
        &app,
        authed_post("/transactions", &session, &csrf, body.clone()),
    )
    .await;
    assert_eq!(status1, StatusCode::CREATED, "first send: {body1}");
    let resp1: serde_json::Value = serde_json::from_str(&body1).unwrap();
    let id1 = resp1["id"].as_str().unwrap().to_string();
    let cents1 = resp1["cents"].as_i64().unwrap();

    // Second send with the same client_id: returns the existing row (idempotent).
    let (status2, body2) = send(&app, authed_post("/transactions", &session, &csrf, body)).await;
    assert!(
        status2 == StatusCode::CREATED || status2 == StatusCode::OK,
        "second send status: {status2}, body: {body2}"
    );
    let resp2: serde_json::Value = serde_json::from_str(&body2).unwrap();
    let id2 = resp2["id"].as_str().unwrap().to_string();
    let cents2 = resp2["cents"].as_i64().unwrap();

    // Same row returned both times.
    assert_eq!(id1, id2, "idempotent replay must return the same row id");
    assert_eq!(cents1, cents2, "amount must be unchanged");

    // Verify exactly one row exists in the database.
    let row_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM transactions WHERE client_id = $1")
            .bind(client_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        row_count.0, 1,
        "exactly one row must exist for the client_id"
    );
}

/// A different client_id creates a new row (not a duplicate).
#[tokio::test]
async fn different_client_ids_create_separate_rows() {
    let (app, _, pool) = build_test_router().await;
    let (session, csrf) = login_as(&app, TEST_USER).await;
    let account_id = create_account(&app, &session, &csrf).await;

    let make_body = |cid: &str| {
        serde_json::to_vec(&serde_json::json!({
            "account_id": account_id,
            "date": "2024-07-01",
            "cents": -1000,
            "client_id": cid
        }))
        .unwrap()
    };

    // Send with client_id A.
    let (status, body) = send(
        &app,
        authed_post("/transactions", &session, &csrf, make_body("cid-a")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "first: {body}");

    // Send with a different client_id B.
    let (status, body) = send(
        &app,
        authed_post("/transactions", &session, &csrf, make_body("cid-b")),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "second (different cid): {body}"
    );

    // Two rows should exist.
    let count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM transactions WHERE client_id IN ('cid-a', 'cid-b')")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count.0, 2);
}

#[tokio::test]
async fn suggest_descriptions_groups_and_requires_three_chars() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let account_id = create_account(&app, &session, &csrf).await;

    for (date, cents, desc) in [
        ("2024-06-01", -1000, "Spesa Conad"),
        ("2024-06-05", -2500, "spesa conad"),
        ("2024-06-07", -900, "Benzina"),
    ] {
        let body = serde_json::to_vec(&serde_json::json!({
            "account_id": account_id, "date": date, "cents": cents, "description": desc
        }))
        .unwrap();
        let (status, _) = send(&app, authed_post("/transactions", &session, &csrf, body)).await;
        assert_eq!(status, StatusCode::CREATED);
    }

    // Too short: empty answer, no query.
    let (status, body_str) = send(&app, authed_get("/transactions/suggest?q=sp", &session)).await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    assert_eq!(body_str.trim(), "[]");

    // Case-insensitive match, grouped, amount of the most recent use.
    let (status, body_str) = send(&app, authed_get("/transactions/suggest?q=CONA", &session)).await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let list = parsed.as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["description"], "spesa conad");
    assert_eq!(list[0]["cents"], -2500);
    assert_eq!(list[0]["count"], 2);

    // LIKE wildcards are literal.
    let (_, body_str) = send(
        &app,
        authed_get("/transactions/suggest?q=%25%25%25", &session),
    )
    .await;
    assert_eq!(body_str.trim(), "[]");

    // The sign narrows the answer: an inflow with the same description only
    // shows up for `sign=1`, and the outflow group for `sign=-1`.
    let body = serde_json::to_vec(&serde_json::json!({
        "account_id": account_id, "date": "2024-06-09", "cents": 700, "description": "Spesa Conad"
    }))
    .unwrap();
    let (status, _) = send(&app, authed_post("/transactions", &session, &csrf, body)).await;
    assert_eq!(status, StatusCode::CREATED);
    for (sign, cents, count) in [(1, 700, 1), (-1, -2500, 2)] {
        let (status, body_str) = send(
            &app,
            authed_get(
                &format!("/transactions/suggest?q=conad&sign={sign}"),
                &session,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {body_str}");
        let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
        let list = parsed.as_array().unwrap();
        assert_eq!(list.len(), 1, "sign {sign}: {body_str}");
        assert_eq!(list[0]["cents"], cents);
        assert_eq!(list[0]["count"], count);
    }
}

// ─── tags ───────────────────────────────────────────────────────────────────

async fn json_of(app: &Router, req: Request<Body>) -> (StatusCode, serde_json::Value) {
    let (s, b) = send(app, req).await;
    (
        s,
        serde_json::from_str(&b).unwrap_or(serde_json::Value::Null),
    )
}

#[tokio::test]
async fn tags_crud_assign_and_bulk() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let account_id = create_account(&app, &session, &csrf).await;

    let mut ids = Vec::new();
    for day in ["2024-06-15", "2024-06-16"] {
        let body = serde_json::to_vec(&serde_json::json!({
            "account_id": account_id, "date": day, "cents": -1000,
            "category_id": "cat-fallback-out"
        }))
        .unwrap();
        let (_, v) = json_of(&app, authed_post("/transactions", &session, &csrf, body)).await;
        ids.push(v["id"].as_str().unwrap().to_string());
    }

    // Create two tags; a duplicate label (any case) is rejected.
    let mk = |label: &str| {
        serde_json::to_vec(&serde_json::json!({
            "label": label, "description": "d", "color": "#ff0000"
        }))
        .unwrap()
    };
    let (st, vacanza) = json_of(&app, authed_post("/tags", &session, &csrf, mk("Vacanza"))).await;
    assert_eq!(st, StatusCode::CREATED);
    let (_, lavoro) = json_of(&app, authed_post("/tags", &session, &csrf, mk("Lavoro"))).await;
    let (st, _) = json_of(&app, authed_post("/tags", &session, &csrf, mk("vacanza"))).await;
    assert_eq!(st, StatusCode::CONFLICT);
    let (st, _) = json_of(
        &app,
        authed_post(
            "/tags",
            &session,
            &csrf,
            serde_json::to_vec(&serde_json::json!({"label": "x", "color": "red"})).unwrap(),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    let (v, l) = (
        vacanza["id"].as_str().unwrap(),
        lavoro["id"].as_str().unwrap(),
    );

    // Several tags on one transaction.
    let body = serde_json::to_vec(&serde_json::json!({ "tag_ids": [v, l] })).unwrap();
    let (st, _) = send(
        &app,
        authed_put(
            &format!("/transactions/{}/tags", ids[0]),
            &session,
            &csrf,
            body,
        ),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    // Bulk: add Vacanza to both (no duplicate on the first), remove Lavoro.
    let body =
        serde_json::to_vec(&serde_json::json!({ "ids": ids, "add": [v], "remove": [l] })).unwrap();
    let (st, _) = send(
        &app,
        authed_post("/transactions/bulk-tags", &session, &csrf, body),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    let (_, map) = json_of(&app, authed_get("/tags/assignments", &session)).await;
    assert_eq!(map[&ids[0]], serde_json::json!([v]));
    assert_eq!(map[&ids[1]], serde_json::json!([v]));

    let (_, list) = json_of(&app, authed_get("/tags", &session)).await;
    let uses = |label: &str| {
        list.as_array()
            .unwrap()
            .iter()
            .find(|t| t["label"] == label)
            .unwrap()["uses"]
            .clone()
    };
    assert_eq!(
        (uses("Vacanza"), uses("Lavoro")),
        (serde_json::json!(2), serde_json::json!(0))
    );

    // Deleting a transaction or a tag leaves no dangling assignment.
    send(
        &app,
        authed_delete(&format!("/transactions/{}", ids[1]), &session, &csrf),
    )
    .await;
    send(&app, authed_delete(&format!("/tags/{v}"), &session, &csrf)).await;
    let (_, map) = json_of(&app, authed_get("/tags/assignments", &session)).await;
    assert_eq!(map, serde_json::json!({}));
}
