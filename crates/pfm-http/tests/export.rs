//! HTTP integration tests for the `POST /export` endpoint.

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

    // ─── schema ──────────────────────────────────────────────────────────

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

    // u3: shared user (can see u1's account)
    sqlx::query(
        "INSERT INTO users (id, username, email, password_hash, role, active, locale, default_currency, created_at)
         VALUES ('u3', $1, 'shared@example.com', $2, 'user', 1, 'en', 'USD', '2025-01-01T00:00:00Z')",
    )
    .bind("shareduser")
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
        .merge(pfm_http::export_routes::export_routes())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            csrf_middleware,
        ))
        .layer(session_layer)
        .with_state(state.clone());

    (app, state, pool_owned)
}

// ─── helpers ──────────────────────────────────────────────────────────────────

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

/// Build an authenticated POST request with CSRF. Query string is in `uri`.
fn authed_post_query(uri: &str, session: &str, csrf: &str) -> Request<Body> {
    let mut req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("X-CSRF-Token", csrf)
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr()));
    req.headers_mut()
        .insert(header::COOKIE, session.parse().unwrap());
    req
}

async fn send_full(
    app: &Router,
    req: Request<Body>,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let res = app.clone().oneshot(req).await.expect("request failed");
    let status = res.status();
    let headers = res.headers().clone();
    let body = axum::body::to_bytes(res.into_body(), 1_048_576)
        .await
        .unwrap();
    (status, headers, String::from_utf8_lossy(&body).into_owned())
}

async fn login(app: &Router) -> (String, String) {
    let body =
        serde_json::to_vec(&serde_json::json!({ "username": TEST_USER, "password": TEST_PASS }))
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
    let body =
        serde_json::to_vec(&serde_json::json!({ "username": username, "password": TEST_PASS }))
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

/// Create an account and return its ID.
async fn create_account(app: &Router, session: &str, csrf: &str) -> String {
    let body = serde_json::to_vec(&serde_json::json!({
        "name": "Export Test",
        "kind": "current",
        "icon": "bi-wallet",
        "start_date": "2024-01-01",
        "start_balance_cents": 10000,
    }))
    .unwrap();

    let mut req = Request::builder()
        .method("POST")
        .uri("/accounts")
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-CSRF-Token", csrf)
        .body(Body::from(body))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr()));
    req.headers_mut()
        .insert(header::COOKIE, session.parse().unwrap());

    let res = app.clone().oneshot(req).await.expect("request failed");
    assert_eq!(res.status(), StatusCode::CREATED);
    let b = axum::body::to_bytes(res.into_body(), 1_048_576)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&b).unwrap();
    v["id"].as_str().unwrap().to_string()
}

/// Seed a transaction directly in the database.
async fn seed_tx(
    pool: &sqlx::AnyPool,
    id: &str,
    account: &str,
    date: &str,
    cents: i64,
    desc: &str,
) {
    sqlx::query(
        "INSERT INTO transactions (id, client_id, account_id, date, cents, description, category_id, created_by, created_at, updated_at)
         VALUES ($1, $1, $2, $3, $4, $5, NULL, 'u1', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
    )
    .bind(id)
    .bind(account)
    .bind(date)
    .bind(cents)
    .bind(desc)
    .execute(pool)
    .await
    .unwrap();
}

// ─── tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn export_returns_csv_with_content_disposition() {
    let (app, _state, pool) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let account_id = create_account(&app, &session, &csrf).await;

    // seed one transaction
    seed_tx(&pool, "tx1", &account_id, "2024-01-15", -500, "Lunch").await;

    let uri = format!(
        "/export?from=2024-01-01&to=2024-01-31&account={}",
        account_id
    );
    let (status, headers, body) = send_full(&app, authed_post_query(&uri, &session, &csrf)).await;

    assert_eq!(status, StatusCode::OK, "body: {body}");

    // Content-Type
    let ct = headers.get(header::CONTENT_TYPE).unwrap().to_str().unwrap();
    assert!(
        ct.contains("text/csv"),
        "Content-Type should be text/csv, got: {ct}"
    );
    assert!(
        ct.contains("charset=utf-8"),
        "Content-Type should have charset, got: {ct}"
    );

    // Content-Disposition
    let cd = headers
        .get(header::CONTENT_DISPOSITION)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(cd.contains("attachment"), "Content-Disposition: {cd}");
    assert!(cd.contains("filename="), "Content-Disposition: {cd}");
    assert!(
        cd.contains(".csv"),
        "Content-Disposition should have .csv: {cd}"
    );

    // Body: BOM + header
    assert!(body.starts_with('\u{feff}'), "body must start with BOM");
    let stripped = body.strip_prefix('\u{feff}').unwrap();
    let lines: Vec<&str> = stripped.split("\r\n").filter(|l| !l.is_empty()).collect();
    assert_eq!(
        lines[0], "Data;Descrizione;Categoria;Importo;Saldo",
        "header line"
    );
    assert_eq!(lines.len(), 2, "1 header + 1 data row");

    // Data row: date dd/mm/aaaa, description, category, amount (comma decimal), balance
    let data_row = lines[1];
    assert!(
        data_row.contains("15/01/2024"),
        "date should be dd/mm/aaaa: {data_row}"
    );
    assert!(
        data_row.contains("Lunch"),
        "description should be present: {data_row}"
    );
    assert!(
        data_row.contains("-5,00"),
        "amount should be -5,00: {data_row}"
    );
}

#[tokio::test]
async fn export_forbidden_for_unshared_user() {
    let (app, _state, pool) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let account_id = create_account(&app, &session, &csrf).await;

    // seed a tx so there's data
    seed_tx(&pool, "tx1", &account_id, "2024-01-15", -500, "test").await;

    // otheruser has no share on this account
    let (other_session, other_csrf) = login_as(&app, OTHER_USER).await;

    let uri = format!(
        "/export?from=2024-01-01&to=2024-01-31&account={}",
        account_id
    );
    let (status, _headers, body) =
        send_full(&app, authed_post_query(&uri, &other_session, &other_csrf)).await;

    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body}");
}

#[tokio::test]
async fn export_requires_auth() {
    let (app, _state, _pool) = build_test_router().await;

    // no session cookie → rejected (400 by CSRF or 401 by auth)
    let mut req = Request::builder()
        .method("POST")
        .uri("/export?from=2024-01-01&to=2024-01-31&account=x")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr()));

    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    assert!(
        status == StatusCode::UNAUTHORIZED || status == StatusCode::BAD_REQUEST,
        "expected 400 or 401 for unauthenticated export, got {}",
        status
    );
}

#[tokio::test]
async fn export_applies_filters() {
    let (app, _state, pool) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let account_id = create_account(&app, &session, &csrf).await;

    // seed: one food (-500), one travel (-2000)
    seed_tx(&pool, "tx1", &account_id, "2024-01-10", -500, "Lunch").await;
    seed_tx(&pool, "tx2", &account_id, "2024-01-20", -2000, "Flight").await;

    // filter by text "Lunch"
    let uri = format!(
        "/export?from=2024-01-01&to=2024-01-31&account={}&q=Lunch",
        account_id
    );
    let (status, _headers, body) = send_full(&app, authed_post_query(&uri, &session, &csrf)).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let stripped = body.strip_prefix('\u{feff}').unwrap();
    let lines: Vec<&str> = stripped.split("\r\n").filter(|l| !l.is_empty()).collect();
    // header + 1 row (only Lunch matches the text filter)
    assert_eq!(lines.len(), 2, "filter q=Lunch should keep only 1 row");
    assert!(lines[1].contains("Lunch"));
    assert!(!lines[1].contains("Flight"));
}

#[tokio::test]
async fn export_csv_roundtrip_no_precision_loss() {
    let (app, _state, pool) = build_test_router().await;
    let (session, csrf) = login(&app).await;
    let account_id = create_account(&app, &session, &csrf).await;

    // seed with various cent values
    seed_tx(&pool, "tx1", &account_id, "2024-01-01", -1234, "one").await;
    seed_tx(&pool, "tx2", &account_id, "2024-01-02", 5678, "two").await;
    seed_tx(&pool, "tx3", &account_id, "2024-01-03", -1, "three").await;

    let uri = format!(
        "/export?from=2024-01-01&to=2024-01-31&account={}",
        account_id
    );
    let (status, _headers, body) = send_full(&app, authed_post_query(&uri, &session, &csrf)).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let stripped = body.strip_prefix('\u{feff}').unwrap();
    let lines: Vec<&str> = stripped.split("\r\n").filter(|l| !l.is_empty()).collect();
    let data_rows = &lines[1..];
    assert_eq!(data_rows.len(), 3);

    // Verify the amounts in the CSV round-trip exactly
    let expected_amounts = ["-12,34", "56,78", "-0,01"];
    for (i, (row, expected)) in data_rows.iter().zip(expected_amounts.iter()).enumerate() {
        assert!(
            row.contains(expected),
            "row {} should contain '{}': {}",
            i,
            expected,
            row
        );
    }
}

#[tokio::test]
async fn export_bad_date_returns_422() {
    let (app, _state, _pool) = build_test_router().await;
    let (session, csrf) = login(&app).await;

    let uri = "/export?from=not-a-date&to=2024-01-31&account=x";
    let (status, _headers, body) = send_full(&app, authed_post_query(uri, &session, &csrf)).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
}
