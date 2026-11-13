//! HTTP integration tests for the categories routes.

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

// ─── categories: requires auth ────────────────────────────────────────────────

#[tokio::test]
async fn categories_requires_session() {
    let (app, _) = build_test_router().await;
    let (status, _) = send(&app, authed_get("/categories", "")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ─── categories: list ─────────────────────────────────────────────────────────

#[tokio::test]
async fn list_includes_fallback_categories() {
    let (app, _) = build_test_router().await;
    let (session, _) = login(&app).await;

    let (status, body_str) = send(&app, authed_get("/categories", &session)).await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let arr = parsed.as_array().unwrap();
    assert_eq!(arr.len(), 2, "should have exactly 2 fallback categories");

    let names: Vec<&str> = arr.iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"Varie"));
    assert!(names.contains(&"Altre entrate"));
}

// ─── categories: create ───────────────────────────────────────────────────────

#[tokio::test]
async fn create_category() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "name": "Ristoranti",
        "emoji": "🍕",
        "kind": "out",
        "budget_cents": 20000
    }))
    .unwrap();

    let (status, body_str) = send(&app, authed_post("/categories", &session, &csrf, body)).await;
    assert_eq!(status, StatusCode::CREATED, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert!(!parsed["id"].as_str().unwrap().is_empty());
    // Owned categories carry an owner suffix so two users can reuse a name.
    assert_eq!(parsed["key"], "ristoranti-u1");
    assert_eq!(parsed["name"], "Ristoranti");
    assert_eq!(parsed["emoji"], "🍕");
    assert_eq!(parsed["kind"], "out");
    assert_eq!(parsed["budget_cents"], 20000);
    assert_eq!(parsed["is_fallback"], false);
}

#[tokio::test]
async fn create_category_empty_name_rejected() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "name": "   ",
        "emoji": "📦",
        "kind": "out"
    }))
    .unwrap();

    let (status, body_str) = send(&app, authed_post("/categories", &session, &csrf, body)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body_str}");
}

#[tokio::test]
async fn create_category_empty_emoji_rejected() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "name": "Spesa",
        "emoji": "",
        "kind": "out"
    }))
    .unwrap();

    let (status, body_str) = send(&app, authed_post("/categories", &session, &csrf, body)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body_str}");
}

#[tokio::test]
async fn create_category_special_chars_slugified() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "name": "Trasporti & Uscite",
        "emoji": "🚗",
        "kind": "out"
    }))
    .unwrap();

    let (status, body_str) = send(&app, authed_post("/categories", &session, &csrf, body)).await;
    assert_eq!(status, StatusCode::CREATED, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed["key"], "trasporti-uscite-u1");
}

#[tokio::test]
async fn create_invalid_kind_rejected() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "name": "Test",
        "emoji": "📦",
        "kind": "savings"
    }))
    .unwrap();

    let (status, body_str) = send(&app, authed_post("/categories", &session, &csrf, body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body_str}");
}

// ─── categories: get ──────────────────────────────────────────────────────────

#[tokio::test]
async fn get_category_by_id() {
    let (app, _) = build_test_router().await;
    let (session, _) = login(&app).await;

    // The fallback category has a known id
    let (status, body_str) = send(&app, authed_get("/categories/cat-fallback-out", &session)).await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed["id"], "cat-fallback-out");
    assert_eq!(parsed["is_fallback"], true);
}

#[tokio::test]
async fn get_missing_category_returns_404() {
    let (app, _) = build_test_router().await;
    let (session, _) = login(&app).await;

    let (status, _) = send(&app, authed_get("/categories/nonexistent", &session)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ─── categories: update ───────────────────────────────────────────────────────

#[tokio::test]
async fn update_category_name_and_budget() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    // Create a category first
    let body = serde_json::to_vec(&serde_json::json!({
        "name": "Spesa",
        "emoji": "🛒",
        "kind": "out"
    }))
    .unwrap();
    let (_, body_str) = send(&app, authed_post("/categories", &session, &csrf, body)).await;
    let created: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let id = created["id"].as_str().unwrap().to_string();

    // Update name and budget
    let update_body = serde_json::to_vec(&serde_json::json!({
        "name": "Supermercato",
        "budget_cents": 50000
    }))
    .unwrap();
    let (status, body_str) = send(
        &app,
        authed_put(&format!("/categories/{id}"), &session, &csrf, update_body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed["name"], "Supermercato");
    assert_eq!(parsed["budget_cents"], 50000);
    // Key is immutable
    assert_eq!(parsed["key"], "spesa-u1");
}

#[tokio::test]
async fn update_category_kind_immutable() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "name": "Test",
        "emoji": "🧪",
        "kind": "out"
    }))
    .unwrap();
    let (_, body_str) = send(&app, authed_post("/categories", &session, &csrf, body)).await;
    let created: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let id = created["id"].as_str().unwrap().to_string();

    // Attempt to change kind via PUT — field is not in CategoryUpdateRequest, so it's ignored
    let update_body =
        serde_json::to_vec(&serde_json::json!({ "name": "Cambiato", "kind": "in" })).unwrap();
    let (status, body_str) = send(
        &app,
        authed_put(&format!("/categories/{id}"), &session, &csrf, update_body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed["kind"], "out", "kind must be immutable");
    assert_eq!(parsed["name"], "Cambiato");
}

#[tokio::test]
async fn update_missing_category_returns_404() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let body = serde_json::to_vec(&serde_json::json!({ "name": "x" })).unwrap();
    let (status, _) = send(&app, authed_put("/categories/ghost", &session, &csrf, body)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ─── categories: delete ───────────────────────────────────────────────────────

#[tokio::test]
async fn delete_user_category_succeeds() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "name": "Temp",
        "emoji": "🗑️",
        "kind": "out"
    }))
    .unwrap();
    let (_, body_str) = send(&app, authed_post("/categories", &session, &csrf, body)).await;
    let created: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let id = created["id"].as_str().unwrap().to_string();

    let (status, _) = send(
        &app,
        authed_delete(&format!("/categories/{id}"), &session, &csrf),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Verify gone
    let (status, _) = send(&app, authed_get(&format!("/categories/{id}"), &session)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_fallback_category_rejected() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let (status, body_str) = send(
        &app,
        authed_delete("/categories/cat-fallback-out", &session, &csrf),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "body: {body_str}");
    assert!(body_str.contains("fallback"));
}

#[tokio::test]
async fn delete_missing_category_returns_404() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let (status, _) = send(&app, authed_delete("/categories/ghost", &session, &csrf)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
