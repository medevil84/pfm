//! HTTP integration tests for the auth routes.
//!
//! Each test spins up an in-memory SQLite database with a single test user,
//! builds the full router (session layer + CSRF middleware + auth routes),
//! and issues requests via `tower::ServiceExt::oneshot`.

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

/// Build a fully-wired test router with an in-memory SQLite DB and one user.
async fn build_test_router() -> (Router, Arc<AppState>) {
    sqlx::any::install_default_drivers();

    // -- config
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
                per_min: 3,
                window_s: 60,
            },
            ..Default::default()
        },
        ..Default::default()
    };
    let config = Arc::new(config);

    // -- storage
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

    // -- schema + seed user
    // Real schema, from the migrations the application actually runs — indexes
    // included. The hand-written DDL this replaces defined no indexes at all,
    // so constraint violations that break production went unnoticed here.
    let migrator = sqlx::migrate!("../../migrations");
    storage.migrate(&migrator).await.expect("migrations");

    let pool = storage.pool();

    // hash the test password and insert the user
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

    // -- services
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

    // -- router
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

/// Helper: build a request with `ConnectInfo` injected into extensions.
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

/// Helper: login and return (session_cookie, csrf_token).
async fn login(app: &Router) -> (String, String) {
    let body = serde_json::to_vec(&serde_json::json!({
        "username": TEST_USER,
        "password": TEST_PASS
    }))
    .unwrap();
    let (status, body_str, headers) =
        send(app, req_with_connect_info("POST", "/auth/login", body)).await;
    assert_eq!(status, StatusCode::OK, "login failed: {body_str}");

    // extract session cookie
    let session_cookie = headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|c| c.starts_with("pfm_session="))
        .expect("no session cookie")
        .to_string();

    // extract csrf token from JSON body
    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let csrf_token = parsed["csrf_token"].as_str().unwrap().to_string();

    (session_cookie, csrf_token)
}

// ─── health probes ────────────────────────────────────────────────────────────

#[tokio::test]
async fn healthz_ok() {
    let (app, _) = build_test_router().await;
    let (status, _, _) = send(&app, req_with_connect_info("GET", "/healthz", vec![])).await;
    assert_eq!(status, StatusCode::OK);
}

// ─── login ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn login_success_returns_csrf_and_session() {
    let (app, _) = build_test_router().await;
    let (session_cookie, csrf_token) = login(&app).await;

    assert!(!session_cookie.is_empty());
    assert!(!csrf_token.is_empty());
    assert_eq!(csrf_token.len(), 64, "csrf token should be 64 hex chars");
}

#[tokio::test]
async fn login_wrong_password_rejected() {
    let (app, _) = build_test_router().await;
    let body = serde_json::to_vec(&serde_json::json!({
        "username": TEST_USER,
        "password": "wrong"
    }))
    .unwrap();
    let (status, body_str, _) =
        send(&app, req_with_connect_info("POST", "/auth/login", body)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body_str.contains("invalid"), "body: {body_str}");
}

#[tokio::test]
async fn login_unknown_user_rejected() {
    let (app, _) = build_test_router().await;
    let body = serde_json::to_vec(&serde_json::json!({
        "username": "nobody",
        "password": "whatever"
    }))
    .unwrap();
    let (status, _, _) = send(&app, req_with_connect_info("POST", "/auth/login", body)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_rate_limited_after_threshold() {
    let (app, _) = build_test_router().await;
    let body = serde_json::to_vec(&serde_json::json!({
        "username": TEST_USER,
        "password": "wrong"
    }))
    .unwrap();

    // per_min = 3, so 3 attempts should succeed, 4th should be 429
    for i in 0..3 {
        let (status, _, _) = send(
            &app,
            req_with_connect_info("POST", "/auth/login", body.clone()),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "attempt {i} should be 401"
        );
    }

    let (status, _, _) = send(&app, req_with_connect_info("POST", "/auth/login", body)).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

// ─── /auth/me ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn me_requires_session() {
    let (app, _) = build_test_router().await;
    let (status, _, _) = send(&app, req_with_connect_info("GET", "/auth/me", vec![])).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn me_returns_user_after_login() {
    let (app, _) = build_test_router().await;
    let (session_cookie, _) = login(&app).await;

    let mut req = Request::builder()
        .method("GET")
        .uri("/auth/me")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo("127.0.0.1:9999".parse::<SocketAddr>().unwrap()));
    req.headers_mut()
        .insert(header::COOKIE, session_cookie.parse().unwrap());

    let (status, body_str, _) = send(&app, req).await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed["user"]["username"], TEST_USER);
    assert_eq!(parsed["user"]["role"], "user");
}

// ─── logout ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn logout_returns_no_content() {
    let (app, _) = build_test_router().await;
    let (session_cookie, csrf_token) = login(&app).await;

    let mut req = Request::builder()
        .method("POST")
        .uri("/auth/logout")
        .header("X-CSRF-Token", &csrf_token)
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo("127.0.0.1:9999".parse::<SocketAddr>().unwrap()));
    req.headers_mut()
        .insert(header::COOKIE, session_cookie.parse().unwrap());

    let (status, _, _) = send(&app, req).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

// ─── CSRF protection ──────────────────────────────────────────────────────────

#[tokio::test]
async fn csrf_missing_header_rejected() {
    let (app, _) = build_test_router().await;
    let (session_cookie, _) = login(&app).await;

    let mut req = Request::builder()
        .method("PUT")
        .uri("/auth/prefs")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({
                "theme": "dark",
                "show_sign": true,
                "privacy": false,
                "show_m_saldo": false,
                "empty_days": 0,
                "show_future": true,
                "rows_above": 3,
                "rows_below": 10,
                "rows_step": 1
            }))
            .unwrap(),
        ))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo("127.0.0.1:9999".parse::<SocketAddr>().unwrap()));
    req.headers_mut()
        .insert(header::COOKIE, session_cookie.parse().unwrap());
    // no X-CSRF-Token header

    let (status, body_str, _) = send(&app, req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body_str}");
}

#[tokio::test]
async fn csrf_mismatch_rejected() {
    let (app, _) = build_test_router().await;
    let (session_cookie, _) = login(&app).await;

    let mut req = Request::builder()
        .method("PUT")
        .uri("/auth/prefs")
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-CSRF-Token", "wrong-token")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({
                "theme": "dark",
                "show_sign": true,
                "privacy": false,
                "show_m_saldo": false,
                "empty_days": 0,
                "show_future": true,
                "rows_above": 3,
                "rows_below": 10,
                "rows_step": 1
            }))
            .unwrap(),
        ))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo("127.0.0.1:9999".parse::<SocketAddr>().unwrap()));
    req.headers_mut()
        .insert(header::COOKIE, session_cookie.parse().unwrap());

    let (status, body_str, _) = send(&app, req).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body_str}");
}

#[tokio::test]
async fn csrf_correct_token_accepted() {
    let (app, _) = build_test_router().await;
    let (session_cookie, csrf_token) = login(&app).await;

    let mut req = Request::builder()
        .method("PUT")
        .uri("/auth/prefs")
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-CSRF-Token", &csrf_token)
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({
                "theme": "dark",
                "show_sign": true,
                "privacy": false,
                "show_m_saldo": false,
                "empty_days": 0,
                "show_future": true,
                "rows_above": 3,
                "rows_below": 10,
                "rows_step": 1
            }))
            .unwrap(),
        ))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo("127.0.0.1:9999".parse::<SocketAddr>().unwrap()));
    req.headers_mut()
        .insert(header::COOKIE, session_cookie.parse().unwrap());

    let (status, _, _) = send(&app, req).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

// ─── password change ──────────────────────────────────────────────────────────

#[tokio::test]
async fn change_password_success() {
    let (app, _) = build_test_router().await;
    let (session_cookie, csrf_token) = login(&app).await;

    let mut req = Request::builder()
        .method("PUT")
        .uri("/auth/password")
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-CSRF-Token", &csrf_token)
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({
                "current_password": TEST_PASS,
                "new_password": "new-password-123"
            }))
            .unwrap(),
        ))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo("127.0.0.1:9999".parse::<SocketAddr>().unwrap()));
    req.headers_mut()
        .insert(header::COOKIE, session_cookie.parse().unwrap());

    let (status, body_str, _) = send(&app, req).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "body: {body_str}");
}

fn locale_req(locale: &str, cookie: Option<&str>, csrf: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("PUT")
        .uri("/auth/locale")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(c) = csrf {
        builder = builder.header("X-CSRF-Token", c);
    }
    let mut req = builder
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({ "locale": locale })).unwrap(),
        ))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo("127.0.0.1:9999".parse::<SocketAddr>().unwrap()));
    if let Some(c) = cookie {
        req.headers_mut().insert(header::COOKIE, c.parse().unwrap());
    }
    req
}

#[tokio::test]
async fn set_locale_success_and_reflected_in_me() {
    let (app, _) = build_test_router().await;
    let (cookie, csrf) = login(&app).await;

    let (status, body, _) = send(&app, locale_req("en", Some(&cookie), Some(&csrf))).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "body: {body}");

    let mut req = Request::builder()
        .method("GET")
        .uri("/auth/me")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo("127.0.0.1:9999".parse::<SocketAddr>().unwrap()));
    req.headers_mut()
        .insert(header::COOKIE, cookie.parse().unwrap());
    let (status, body, _) = send(&app, req).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["user"]["locale"], "en");
}

#[tokio::test]
async fn set_locale_invalid_rejected() {
    let (app, _) = build_test_router().await;
    let (cookie, csrf) = login(&app).await;
    let (status, body, _) = send(&app, locale_req("xx", Some(&cookie), Some(&csrf))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
}

#[tokio::test]
async fn set_locale_requires_session() {
    let (app, _) = build_test_router().await;
    // Without a session the CSRF middleware rejects the mutating request first
    // (400 errors.csrf_missing); the handler's own 401 is not reachable here.
    let (status, _, _) = send(&app, locale_req("en", None, None)).await;
    assert!(
        status == StatusCode::UNAUTHORIZED || status == StatusCode::BAD_REQUEST,
        "unexpected status {status}"
    );
}

#[tokio::test]
async fn change_password_wrong_current_rejected() {
    let (app, _) = build_test_router().await;
    let (session_cookie, csrf_token) = login(&app).await;

    let mut req = Request::builder()
        .method("PUT")
        .uri("/auth/password")
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-CSRF-Token", &csrf_token)
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({
                "current_password": "wrong",
                "new_password": "new-password-123"
            }))
            .unwrap(),
        ))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo("127.0.0.1:9999".parse::<SocketAddr>().unwrap()));
    req.headers_mut()
        .insert(header::COOKIE, session_cookie.parse().unwrap());

    let (status, body_str, _) = send(&app, req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body_str}");
}

// ─── preferences ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn get_prefs_returns_defaults() {
    let (app, _) = build_test_router().await;
    let (session_cookie, _) = login(&app).await;

    let mut req = Request::builder()
        .method("GET")
        .uri("/auth/prefs")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo("127.0.0.1:9999".parse::<SocketAddr>().unwrap()));
    req.headers_mut()
        .insert(header::COOKIE, session_cookie.parse().unwrap());

    let (status, body_str, _) = send(&app, req).await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed["theme"], "system");
    assert_eq!(parsed["rows_above"], 2);
    assert_eq!(parsed["rows_below"], 8);
}

#[tokio::test]
async fn update_then_get_prefs_roundtrip() {
    let (app, _) = build_test_router().await;
    let (session_cookie, csrf_token) = login(&app).await;

    // update
    let prefs = serde_json::json!({
        "theme": "dark",
        "show_sign": true,
        "privacy": true,
        "show_m_saldo": true,
        "empty_days": 5,
        "show_future": true,
        "rows_above": 7,
        "rows_below": 14,
        "rows_step": 2
    });

    let mut update_req = Request::builder()
        .method("PUT")
        .uri("/auth/prefs")
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-CSRF-Token", &csrf_token)
        .body(Body::from(serde_json::to_vec(&prefs).unwrap()))
        .unwrap();
    update_req
        .extensions_mut()
        .insert(ConnectInfo("127.0.0.1:9999".parse::<SocketAddr>().unwrap()));
    update_req
        .headers_mut()
        .insert(header::COOKIE, session_cookie.parse().unwrap());

    let (status, body_str, _) = send(&app, update_req).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "update failed: {body_str}");

    // get
    let mut get_req = Request::builder()
        .method("GET")
        .uri("/auth/prefs")
        .body(Body::empty())
        .unwrap();
    get_req
        .extensions_mut()
        .insert(ConnectInfo("127.0.0.1:9999".parse::<SocketAddr>().unwrap()));
    get_req
        .headers_mut()
        .insert(header::COOKIE, session_cookie.parse().unwrap());

    let (status, body_str, _) = send(&app, get_req).await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed["theme"], "dark");
    assert_eq!(parsed["show_sign"], true);
    assert_eq!(parsed["privacy"], true);
    assert_eq!(parsed["show_m_saldo"], true);
    assert_eq!(parsed["empty_days"], 5);
    assert_eq!(parsed["show_future"], true);
    assert_eq!(parsed["rows_above"], 7);
    assert_eq!(parsed["rows_below"], 14);
    assert_eq!(parsed["rows_step"], 2);
}

/// PUT `/auth/prefs` with the given colours and return the status.
async fn put_colors(
    app: &Router,
    session: &str,
    csrf: &str,
    extra: serde_json::Value,
) -> StatusCode {
    let mut prefs = serde_json::json!({
        "theme": "system", "show_sign": true, "privacy": false, "show_m_saldo": false,
        "empty_days": 0, "show_future": true, "rows_above": 2, "rows_below": 8, "rows_step": 1
    });
    prefs
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    let mut req = Request::builder()
        .method("PUT")
        .uri("/auth/prefs")
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-CSRF-Token", csrf)
        .body(Body::from(serde_json::to_vec(&prefs).unwrap()))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo("127.0.0.1:9999".parse::<SocketAddr>().unwrap()));
    req.headers_mut()
        .insert(header::COOKIE, session.parse().unwrap());
    send(app, req).await.0
}

async fn get_prefs_json(app: &Router, session: &str) -> serde_json::Value {
    let mut req = Request::builder()
        .method("GET")
        .uri("/auth/prefs")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo("127.0.0.1:9999".parse::<SocketAddr>().unwrap()));
    req.headers_mut()
        .insert(header::COOKIE, session.parse().unwrap());
    let (status, body, _) = send(app, req).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    serde_json::from_str(&body).unwrap()
}

#[tokio::test]
async fn amount_colors_default_to_empty_then_roundtrip() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    // Default = empty string (the client shows green / red).
    let p = get_prefs_json(&app, &session).await;
    assert_eq!(p["color_pos"], "");
    assert_eq!(p["color_neg"], "");

    let status = put_colors(
        &app,
        &session,
        &csrf,
        serde_json::json!({"color_pos": "#1A9E5C", "color_neg": "#ff00aa"}),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let p = get_prefs_json(&app, &session).await;
    assert_eq!(p["color_pos"], "#1A9E5C");
    assert_eq!(p["color_neg"], "#ff00aa");

    // Sending empty strings (or omitting the fields) restores the defaults.
    let status = put_colors(&app, &session, &csrf, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let p = get_prefs_json(&app, &session).await;
    assert_eq!(p["color_pos"], "");
    assert_eq!(p["color_neg"], "");
}

#[tokio::test]
async fn amount_colors_reject_invalid_values() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    for bad in [
        "red",
        "#fff",
        "#12345g",
        "#1234567",
        "123456",
        "#123456; background:url(x)",
    ] {
        for field in ["color_pos", "color_neg"] {
            let status = put_colors(&app, &session, &csrf, serde_json::json!({ field: bad })).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{field}={bad}");
        }
    }
    // Nothing invalid was stored.
    let p = get_prefs_json(&app, &session).await;
    assert_eq!(p["color_pos"], "");
    assert_eq!(p["color_neg"], "");
}

// ─── read-only methods bypass CSRF ────────────────────────────────────────────

#[tokio::test]
async fn get_without_csrf_token_passes() {
    let (app, _) = build_test_router().await;
    let (session_cookie, _) = login(&app).await;

    let mut req = Request::builder()
        .method("GET")
        .uri("/auth/me")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo("127.0.0.1:9999".parse::<SocketAddr>().unwrap()));
    req.headers_mut()
        .insert(header::COOKIE, session_cookie.parse().unwrap());
    // no X-CSRF-Token — GET should not require it

    let (status, _, _) = send(&app, req).await;
    assert_eq!(status, StatusCode::OK);
}

// ─── login is CSRF-exempt ─────────────────────────────────────────────────────

#[tokio::test]
async fn login_is_csrf_exempt() {
    let (app, _) = build_test_router().await;
    let body = serde_json::to_vec(&serde_json::json!({
        "username": TEST_USER,
        "password": TEST_PASS
    }))
    .unwrap();
    let (status, _, _) = send(&app, req_with_connect_info("POST", "/auth/login", body)).await;
    assert_eq!(status, StatusCode::OK);
}

// ─── session cookie flags ─────────────────────────────────────────────────────

#[tokio::test]
async fn session_cookie_has_httponly_and_secure_matches_config() {
    // trust_proxy is false in test config, so Secure must be absent.
    let (app, _) = build_test_router().await;
    let body = serde_json::to_vec(&serde_json::json!({
        "username": TEST_USER,
        "password": TEST_PASS
    }))
    .unwrap();
    let (status, _, headers) = send(&app, req_with_connect_info("POST", "/auth/login", body)).await;
    assert_eq!(status, StatusCode::OK);

    let session_cookie = headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|c| c.starts_with("pfm_session="))
        .expect("no session cookie in Set-Cookie");

    assert!(
        session_cookie.contains("HttpOnly"),
        "session cookie must be HttpOnly: {session_cookie}"
    );
    assert!(
        !session_cookie.to_lowercase().contains("secure"),
        "session cookie must NOT be Secure when trust_proxy=false: {session_cookie}"
    );
}
