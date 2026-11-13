//! HTTP integration tests for transfer and bulk-operation routes.

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
        .merge(pfm_http::transfer_routes::transfer_routes())
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

// ─── transfers: requires auth ─────────────────────────────────────────────────

#[tokio::test]
async fn transfers_requires_session() {
    let (app, _, _) = build_test_router().await;
    let (status, _) = send(
        &app,
        authed_get("/transfers?from=2024-01-01&to=2024-01-31", ""),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ─── transfers: create ────────────────────────────────────────────────────────

#[tokio::test]
async fn create_transfer() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let from_id = create_account(&app, &session, &csrf).await;
    let to_id = create_account(&app, &session, &csrf).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "from_account_id": from_id,
        "to_account_id": to_id,
        "date": "2024-06-15",
        "cents": 5000,
        "description": "move money",
        "note": "test transfer"
    }))
    .unwrap();

    let (status, body_str) = send(&app, authed_post("/transfers", &session, &csrf, body)).await;
    assert_eq!(status, StatusCode::CREATED, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert!(!parsed["pair_id"].as_str().unwrap().is_empty());
    assert_eq!(parsed["from_account_id"], from_id);
    assert_eq!(parsed["to_account_id"], to_id);
    assert_eq!(parsed["date"], "2024-06-15");
    assert_eq!(parsed["cents"], 5000);
    assert_eq!(parsed["description"], "move money");
    assert_eq!(parsed["note"], "test transfer");
}

#[tokio::test]
async fn create_transfer_negative_cents() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let from_id = create_account(&app, &session, &csrf).await;
    let to_id = create_account(&app, &session, &csrf).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "from_account_id": from_id,
        "to_account_id": to_id,
        "date": "2024-06-15",
        "cents": -5000
    }))
    .unwrap();

    let (status, _) = send(&app, authed_post("/transfers", &session, &csrf, body)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn create_transfer_same_account() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let from_id = create_account(&app, &session, &csrf).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "from_account_id": from_id,
        "to_account_id": from_id,
        "date": "2024-06-15",
        "cents": 5000
    }))
    .unwrap();

    let (status, _) = send(&app, authed_post("/transfers", &session, &csrf, body)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn create_transfer_nonexistent_account() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let to_id = create_account(&app, &session, &csrf).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "from_account_id": "nonexistent",
        "to_account_id": to_id,
        "date": "2024-06-15",
        "cents": 5000
    }))
    .unwrap();

    let (status, _) = send(&app, authed_post("/transfers", &session, &csrf, body)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn create_transfer_invalid_date() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let from_id = create_account(&app, &session, &csrf).await;
    let to_id = create_account(&app, &session, &csrf).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "from_account_id": from_id,
        "to_account_id": to_id,
        "date": "not-a-date",
        "cents": 5000
    }))
    .unwrap();

    let (status, _) = send(&app, authed_post("/transfers", &session, &csrf, body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ─── transfers: get ───────────────────────────────────────────────────────────

#[tokio::test]
async fn get_transfer() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let from_id = create_account(&app, &session, &csrf).await;
    let to_id = create_account(&app, &session, &csrf).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "from_account_id": from_id,
        "to_account_id": to_id,
        "date": "2024-06-15",
        "cents": 3000,
        "description": "fetch"
    }))
    .unwrap();
    let (_, create_str) = send(&app, authed_post("/transfers", &session, &csrf, body)).await;
    let created: serde_json::Value = serde_json::from_str(&create_str).unwrap();
    let pair_id = created["pair_id"].as_str().unwrap().to_string();

    let (status, body_str) =
        send(&app, authed_get(&format!("/transfers/{pair_id}"), &session)).await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed["pair_id"], pair_id);
    assert_eq!(parsed["from_account_id"], from_id);
    assert_eq!(parsed["to_account_id"], to_id);
    assert_eq!(parsed["cents"], 3000);
    assert_eq!(parsed["description"], "fetch");
}

#[tokio::test]
async fn get_transfer_not_found() {
    let (app, _, _) = build_test_router().await;
    let (session, _) = login(&app).await;

    let (status, _) = send(&app, authed_get("/transfers/nonexistent-pair-id", &session)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ─── transfers: update ────────────────────────────────────────────────────────

#[tokio::test]
async fn update_transfer() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let from_id = create_account(&app, &session, &csrf).await;
    let to_id = create_account(&app, &session, &csrf).await;
    let to_id2 = create_account(&app, &session, &csrf).await;

    let create_body = serde_json::to_vec(&serde_json::json!({
        "from_account_id": from_id,
        "to_account_id": to_id,
        "date": "2024-06-15",
        "cents": 3000,
        "description": "original"
    }))
    .unwrap();
    let (_, create_str) = send(
        &app,
        authed_post("/transfers", &session, &csrf, create_body),
    )
    .await;
    let created: serde_json::Value = serde_json::from_str(&create_str).unwrap();
    let pair_id = created["pair_id"].as_str().unwrap().to_string();

    let update_body = serde_json::to_vec(&serde_json::json!({
        "to_account_id": to_id2,
        "cents": 7000,
        "description": "updated"
    }))
    .unwrap();
    let (status, body_str) = send(
        &app,
        authed_put(
            &format!("/transfers/{pair_id}"),
            &session,
            &csrf,
            update_body,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed["pair_id"], pair_id);
    assert_eq!(parsed["from_account_id"], from_id);
    assert_eq!(parsed["to_account_id"], to_id2);
    assert_eq!(parsed["cents"], 7000);
    assert_eq!(parsed["description"], "updated");
}

#[tokio::test]
async fn update_transfer_not_found() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "description": "new"
    }))
    .unwrap();
    let (status, _) = send(
        &app,
        authed_put("/transfers/nonexistent-pair", &session, &csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn update_transfer_same_account_rejected() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let from_id = create_account(&app, &session, &csrf).await;
    let to_id = create_account(&app, &session, &csrf).await;

    let create_body = serde_json::to_vec(&serde_json::json!({
        "from_account_id": from_id,
        "to_account_id": to_id,
        "date": "2024-06-15",
        "cents": 3000
    }))
    .unwrap();
    let (_, create_str) = send(
        &app,
        authed_post("/transfers", &session, &csrf, create_body),
    )
    .await;
    let created: serde_json::Value = serde_json::from_str(&create_str).unwrap();
    let pair_id = created["pair_id"].as_str().unwrap().to_string();

    let body = serde_json::to_vec(&serde_json::json!({
        "to_account_id": from_id
    }))
    .unwrap();
    let (status, _) = send(
        &app,
        authed_put(&format!("/transfers/{pair_id}"), &session, &csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

// ─── transfers: delete ────────────────────────────────────────────────────────

#[tokio::test]
async fn delete_transfer() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let from_id = create_account(&app, &session, &csrf).await;
    let to_id = create_account(&app, &session, &csrf).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "from_account_id": from_id,
        "to_account_id": to_id,
        "date": "2024-06-15",
        "cents": 3000
    }))
    .unwrap();
    let (_, create_str) = send(&app, authed_post("/transfers", &session, &csrf, body)).await;
    let created: serde_json::Value = serde_json::from_str(&create_str).unwrap();
    let pair_id = created["pair_id"].as_str().unwrap().to_string();

    let (status, _) = send(
        &app,
        authed_delete(&format!("/transfers/{pair_id}"), &session, &csrf),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Verify it's gone
    let (status, _) = send(&app, authed_get(&format!("/transfers/{pair_id}"), &session)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_transfer_not_found() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let (status, _) = send(
        &app,
        authed_delete("/transfers/nonexistent-pair", &session, &csrf),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ─── transfers: list ──────────────────────────────────────────────────────────

#[tokio::test]
async fn list_transfers() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let from_id = create_account(&app, &session, &csrf).await;
    let to_id = create_account(&app, &session, &csrf).await;
    let to_id2 = create_account(&app, &session, &csrf).await;

    // Transfer 1: from -> to, 2024-06-01
    let body1 = serde_json::to_vec(&serde_json::json!({
        "from_account_id": from_id,
        "to_account_id": to_id,
        "date": "2024-06-01",
        "cents": 1000
    }))
    .unwrap();
    send(&app, authed_post("/transfers", &session, &csrf, body1)).await;

    // Transfer 2: from -> to2, 2024-06-15
    let body2 = serde_json::to_vec(&serde_json::json!({
        "from_account_id": from_id,
        "to_account_id": to_id2,
        "date": "2024-06-15",
        "cents": 2000
    }))
    .unwrap();
    send(&app, authed_post("/transfers", &session, &csrf, body2)).await;

    // List all transfers in June
    let (status, body_str) = send(
        &app,
        authed_get("/transfers?from=2024-06-01&to=2024-06-30", &session),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let transfers = parsed.as_array().unwrap();
    assert_eq!(transfers.len(), 2);

    // Sorted by date
    assert_eq!(transfers[0]["date"], "2024-06-01");
    assert_eq!(transfers[1]["date"], "2024-06-15");

    // Filter by from_account
    let (status, body_str) = send(
        &app,
        authed_get(
            &format!("/transfers?from=2024-06-01&to=2024-06-30&from_account={from_id}"),
            &session,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed.as_array().unwrap().len(), 2);

    // Filter by to_account (only transfer 1 matches)
    let (status, body_str) = send(
        &app,
        authed_get(
            &format!("/transfers?from=2024-06-01&to=2024-06-30&to_account={to_id}"),
            &session,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn list_transfers_from_after_to() {
    let (app, _, _) = build_test_router().await;
    let (session, _) = login(&app).await;

    let (status, _) = send(
        &app,
        authed_get("/transfers?from=2024-06-30&to=2024-06-01", &session),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ─── transfers: permissions ───────────────────────────────────────────────────

#[tokio::test]
async fn unauthorized_user_cannot_create_transfer() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login_as(&app, TEST_USER).await;
    let (other_session, other_csrf) = login_as(&app, OTHER_USER).await;
    let from_id = create_account(&app, &session, &csrf).await;
    let to_id = create_account(&app, &session, &csrf).await;

    // Other user tries to create a transfer on owner's accounts
    let body = serde_json::to_vec(&serde_json::json!({
        "from_account_id": from_id,
        "to_account_id": to_id,
        "date": "2024-06-15",
        "cents": 5000
    }))
    .unwrap();
    let (status, body_str) = send(
        &app,
        authed_post("/transfers", &other_session, &other_csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body_str}");
}

#[tokio::test]
async fn unauthorized_user_cannot_read_transfer() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login_as(&app, TEST_USER).await;
    let (other_session, _) = login_as(&app, OTHER_USER).await;
    let from_id = create_account(&app, &session, &csrf).await;
    let to_id = create_account(&app, &session, &csrf).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "from_account_id": from_id,
        "to_account_id": to_id,
        "date": "2024-06-15",
        "cents": 5000
    }))
    .unwrap();
    let (_, create_str) = send(&app, authed_post("/transfers", &session, &csrf, body)).await;
    let created: serde_json::Value = serde_json::from_str(&create_str).unwrap();
    let pair_id = created["pair_id"].as_str().unwrap().to_string();

    let (status, body_str) = send(
        &app,
        authed_get(&format!("/transfers/{pair_id}"), &other_session),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body_str}");
}

// ─── bulk: delete ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn bulk_delete() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let account_id = create_account(&app, &session, &csrf).await;

    // Create 3 transactions
    let mut tx_ids = Vec::new();
    for i in 1..=3 {
        let body = serde_json::to_vec(&serde_json::json!({
            "account_id": account_id,
            "date": format!("2024-06-{i:02}"),
            "cents": -1000 * i,
            "description": format!("tx {i}")
        }))
        .unwrap();
        let (_, str) = send(&app, authed_post("/transactions", &session, &csrf, body)).await;
        let parsed: serde_json::Value = serde_json::from_str(&str).unwrap();
        tx_ids.push(parsed["id"].as_str().unwrap().to_string());
    }

    // Bulk delete all 3
    let body = serde_json::to_vec(&serde_json::json!({
        "ids": tx_ids,
        "action": "delete"
    }))
    .unwrap();
    let (status, body_str) = send(
        &app,
        authed_post("/transactions/bulk", &session, &csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed["affected"].as_u64().unwrap(), 3);

    // Verify all are gone
    for id in &tx_ids {
        let (status, _) = send(&app, authed_get(&format!("/transactions/{id}"), &session)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
async fn bulk_delete_empty_ids() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "ids": [],
        "action": "delete"
    }))
    .unwrap();
    let (status, _) = send(
        &app,
        authed_post("/transactions/bulk", &session, &csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn bulk_delete_invalid_action() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "ids": ["some-id"],
        "action": "invalid"
    }))
    .unwrap();
    let (status, _) = send(
        &app,
        authed_post("/transactions/bulk", &session, &csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn bulk_delete_unauthorized() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login_as(&app, TEST_USER).await;
    let (other_session, other_csrf) = login_as(&app, OTHER_USER).await;
    let account_id = create_account(&app, &session, &csrf).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "account_id": account_id,
        "date": "2024-06-15",
        "cents": -1000
    }))
    .unwrap();
    let (_, str) = send(&app, authed_post("/transactions", &session, &csrf, body)).await;
    let parsed: serde_json::Value = serde_json::from_str(&str).unwrap();
    let tx_id = parsed["id"].as_str().unwrap().to_string();

    // Other user tries to bulk delete a tx they don't own
    let body = serde_json::to_vec(&serde_json::json!({
        "ids": [tx_id],
        "action": "delete"
    }))
    .unwrap();
    let (status, body_str) = send(
        &app,
        authed_post("/transactions/bulk", &other_session, &other_csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body_str}");
}

#[tokio::test]
async fn bulk_delete_mixed_permissions_rejected() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login_as(&app, TEST_USER).await;
    let (other_session, other_csrf) = login_as(&app, OTHER_USER).await;
    let account_id = create_account(&app, &session, &csrf).await;

    // Create two txs owned by testuser
    let mut tx_ids = Vec::new();
    for i in 1..=2 {
        let body = serde_json::to_vec(&serde_json::json!({
            "account_id": account_id,
            "date": format!("2024-06-{i:02}"),
            "cents": -1000
        }))
        .unwrap();
        let (_, str) = send(&app, authed_post("/transactions", &session, &csrf, body)).await;
        let parsed: serde_json::Value = serde_json::from_str(&str).unwrap();
        tx_ids.push(parsed["id"].as_str().unwrap().to_string());
    }

    // Other user tries to bulk delete (all txs belong to testuser's account)
    // This should be rejected: other user has no write access to any of these
    let body = serde_json::to_vec(&serde_json::json!({
        "ids": tx_ids,
        "action": "delete"
    }))
    .unwrap();
    let (status, body_str) = send(
        &app,
        authed_post("/transactions/bulk", &other_session, &other_csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body_str}");
}

// ─── bulk: recategorize ───────────────────────────────────────────────────────

#[tokio::test]
async fn bulk_recategorize() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let account_id = create_account(&app, &session, &csrf).await;

    // Create a category
    let cat_body = serde_json::to_vec(&serde_json::json!({
        "name": "Food",
        "emoji": "U+1F374",
        "kind": "out"
    }))
    .unwrap();
    let (_, cat_str) = send(&app, authed_post("/categories", &session, &csrf, cat_body)).await;
    let cat_parsed: serde_json::Value = serde_json::from_str(&cat_str).unwrap();
    let cat_id = cat_parsed["id"].as_str().unwrap().to_string();

    // Create 2 transactions
    let mut tx_ids = Vec::new();
    for i in 1..=2 {
        let body = serde_json::to_vec(&serde_json::json!({
            "account_id": account_id,
            "date": format!("2024-06-{i:02}"),
            "cents": -5000,
            "description": format!("tx {i}")
        }))
        .unwrap();
        let (_, str) = send(&app, authed_post("/transactions", &session, &csrf, body)).await;
        let parsed: serde_json::Value = serde_json::from_str(&str).unwrap();
        tx_ids.push(parsed["id"].as_str().unwrap().to_string());
    }

    // Bulk recategorize
    let body = serde_json::to_vec(&serde_json::json!({
        "ids": tx_ids,
        "action": "recategorize",
        "category_id": cat_id
    }))
    .unwrap();
    let (status, body_str) = send(
        &app,
        authed_post("/transactions/bulk", &session, &csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed["affected"].as_u64().unwrap(), 2);

    // Verify both now have the new category
    for id in &tx_ids {
        let (status, body_str) =
            send(&app, authed_get(&format!("/transactions/{id}"), &session)).await;
        assert_eq!(status, StatusCode::OK, "body: {body_str}");
        let tx: serde_json::Value = serde_json::from_str(&body_str).unwrap();
        assert_eq!(tx["category_id"].as_str().unwrap(), cat_id);
    }
}

#[tokio::test]
async fn bulk_recategorize_unauthorized() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login_as(&app, TEST_USER).await;
    let (other_session, other_csrf) = login_as(&app, OTHER_USER).await;
    let account_id = create_account(&app, &session, &csrf).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "account_id": account_id,
        "date": "2024-06-15",
        "cents": -1000
    }))
    .unwrap();
    let (_, str) = send(&app, authed_post("/transactions", &session, &csrf, body)).await;
    let parsed: serde_json::Value = serde_json::from_str(&str).unwrap();
    let tx_id = parsed["id"].as_str().unwrap().to_string();

    let body = serde_json::to_vec(&serde_json::json!({
        "ids": [tx_id],
        "action": "recategorize",
        "category_id": "cat-fallback-out"
    }))
    .unwrap();
    let (status, body_str) = send(
        &app,
        authed_post("/transactions/bulk", &other_session, &other_csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body_str}");
}

// ─── bulk: nonexistent ids are skipped ────────────────────────────────────────

#[tokio::test]
async fn bulk_delete_nonexistent_ids_skipped() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let account_id = create_account(&app, &session, &csrf).await;

    // Create one real tx
    let body = serde_json::to_vec(&serde_json::json!({
        "account_id": account_id,
        "date": "2024-06-15",
        "cents": -1000
    }))
    .unwrap();
    let (_, str) = send(&app, authed_post("/transactions", &session, &csrf, body)).await;
    let parsed: serde_json::Value = serde_json::from_str(&str).unwrap();
    let tx_id = parsed["id"].as_str().unwrap().to_string();

    // Mix of real and nonexistent ids
    let body = serde_json::to_vec(&serde_json::json!({
        "ids": [tx_id, "nonexistent-1", "nonexistent-2"],
        "action": "delete"
    }))
    .unwrap();
    let (status, body_str) = send(
        &app,
        authed_post("/transactions/bulk", &session, &csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    // Only the real one should be affected
    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed["affected"].as_u64().unwrap(), 1);
}

// ─── gate 2: update leaves no orphans or duplicates ──────────────────────────

/// After `PUT /transfers/{pair_id}` (which deletes and re-creates the pair),
/// the database must contain exactly 2 rows for `pair_id` and none of the
/// original leg IDs may survive (no orphans, no duplicates).
#[tokio::test]
async fn update_transfer_leaves_no_orphan_or_duplicate_legs() {
    let (app, _, pool) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let from_id = create_account(&app, &session, &csrf).await;
    let to_id = create_account(&app, &session, &csrf).await;

    let create_body = serde_json::to_vec(&serde_json::json!({
        "from_account_id": from_id,
        "to_account_id": to_id,
        "date": "2024-06-15",
        "cents": 3000,
        "description": "original"
    }))
    .unwrap();
    let (_, create_str) = send(
        &app,
        authed_post("/transfers", &session, &csrf, create_body),
    )
    .await;
    let created: serde_json::Value = serde_json::from_str(&create_str).unwrap();
    let pair_id = created["pair_id"].as_str().unwrap().to_string();

    // Capture the original leg IDs before the update.
    let old_legs: Vec<String> =
        sqlx::query_scalar("SELECT id FROM transactions WHERE pair_id = $1")
            .bind(&pair_id)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(old_legs.len(), 2, "expected 2 legs after create");
    let old_id_a = old_legs[0].clone();
    let old_id_b = old_legs[1].clone();

    // Update the transfer (changes cents + description).
    let update_body = serde_json::to_vec(&serde_json::json!({
        "cents": 9000,
        "description": "updated"
    }))
    .unwrap();
    let (status, body_str) = send(
        &app,
        authed_put(
            &format!("/transfers/{pair_id}"),
            &session,
            &csrf,
            update_body,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "update failed: {body_str}");

    // Exactly 2 rows for pair_id (no duplicates).
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions WHERE pair_id = $1")
        .bind(&pair_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count, 2,
        "expected exactly 2 legs after update, got {count}"
    );

    // Old leg IDs must be gone (no orphans).
    for (label, old_id) in [("from-leg", &old_id_a), ("to-leg", &old_id_b)] {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions WHERE id = $1")
            .bind(old_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0, "old {label} {old_id} still exists after update");
    }
}

// ─── gate 3: true mixed-permissions bulk rejection ───────────────────────────

/// `testuser` owns tx_a (writable) and references tx_b owned by `otheruser`
/// (not writable by testuser).  A bulk-delete of both must be rejected in
/// full — neither row is deleted.
#[tokio::test]
async fn bulk_delete_true_mixed_permissions_rejects_all() {
    let (app, _, pool) = build_test_router().await;
    let (session, csrf) = login_as(&app, TEST_USER).await;
    let (other_session, other_csrf) = login_as(&app, OTHER_USER).await;

    // testuser: account A + tx_a (writable by testuser).
    let acc_a = create_account(&app, &session, &csrf).await;
    let body_a = serde_json::to_vec(&serde_json::json!({
        "account_id": acc_a,
        "date": "2024-06-01",
        "cents": -1000,
        "description": "tx_a (testuser)"
    }))
    .unwrap();
    let (_, str_a) = send(&app, authed_post("/transactions", &session, &csrf, body_a)).await;
    let parsed_a: serde_json::Value = serde_json::from_str(&str_a).unwrap();
    let tx_a = parsed_a["id"].as_str().unwrap().to_string();

    // otheruser: account B + tx_b (NOT writable by testuser).
    let acc_b = create_account(&app, &other_session, &other_csrf).await;
    let body_b = serde_json::to_vec(&serde_json::json!({
        "account_id": acc_b,
        "date": "2024-06-02",
        "cents": -2000,
        "description": "tx_b (otheruser)"
    }))
    .unwrap();
    let (_, str_b) = send(
        &app,
        authed_post("/transactions", &other_session, &other_csrf, body_b),
    )
    .await;
    let parsed_b: serde_json::Value = serde_json::from_str(&str_b).unwrap();
    let tx_b = parsed_b["id"].as_str().unwrap().to_string();

    // testuser bulk-deletes both: tx_a is writable, tx_b is not → FORBIDDEN.
    let body = serde_json::to_vec(&serde_json::json!({
        "ids": [tx_a, tx_b],
        "action": "delete"
    }))
    .unwrap();
    let (status, body_str) = send(
        &app,
        authed_post("/transactions/bulk", &session, &csrf, body),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "expected FORBIDDEN for mixed-permissions bulk delete: {body_str}"
    );

    // The entire operation was rejected: both rows must still exist.
    for (label, id) in [("tx_a", &tx_a), ("tx_b", &tx_b)] {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1, "{label} {id} was deleted despite FORBIDDEN response");
    }
}
