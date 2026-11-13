//! HTTP integration tests for the spending-rule routes: auth, CRUD under an
//! account, fee-row generation and read-only preview, and the multi-user
//! permission gate (rules are permission-checked against their account).

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
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
const OTHER_USER: &str = "otheruser";

/// Build the test router. Returns the app, its state, and the storage handle so
/// tests that exercise generation/preview can seed a base ledger.
async fn build_test_router() -> (Router, Arc<AppState>, Arc<Storage>) {
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

    // Owner (u1) and an unrelated user (u2) for the permission gate.
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

    // An owned account for rules to reference (after its owner exists).
    sqlx::query(
        "INSERT INTO accounts (id, owner_id, name, kind, start_date, start_balance_cents, active, created_at)
         VALUES ('acc1', 'u1', 'conto', 'current', '2024-01-01', 0, 1, '2025-01-01T00:00:00Z')",
    )
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
        .merge(pfm_http::rule_routes::rule_routes())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            csrf_middleware,
        ))
        .layer(session_layer)
        .with_state(state.clone());

    (app, state, storage)
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

/// Seed a single manual (non-computed) transaction into the base ledger.
static SEED_SEQ: AtomicUsize = AtomicUsize::new(0);

async fn seed_tx(storage: &Storage, date: &str, cents: i64) {
    let id = format!("seed-tx-{}", SEED_SEQ.fetch_add(1, Ordering::Relaxed));
    sqlx::query(
        "INSERT INTO transactions \
         (id, client_id, account_id, date, cents, description, created_by, created_at, updated_at) \
         VALUES ($1, 'c1', 'acc1', $2, $3, 'seed', 'u1', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
    )
    .bind(id)
    .bind(date)
    .bind(cents)
    .execute(storage.pool())
    .await
    .unwrap();
}

/// Create a `pct_avg_balance` rule (1% monthly, charged on the 1st) on acc1 and
/// return its id.
async fn create_pct_rule(app: &Router, session: &str, csrf: &str) -> String {
    let body = serde_json::to_vec(&serde_json::json!({
        "name": "commissione",
        "category_id": "cat-fallback-out",
        "calc_type": "pct_avg_balance",
        "value": 100,
        "cadence": "monthly",
        "charge_day": 1,
        "cond_base": "always",
        "active": true
    }))
    .unwrap();
    let (status, body_str) = send(
        app,
        authed_post("/accounts/acc1/rules", session, csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body_str}");
    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    parsed["id"].as_str().unwrap().to_string()
}

// ─── auth ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn rules_require_session() {
    let (app, _, _) = build_test_router().await;
    let (status, _) = send(&app, authed_get("/accounts/acc1/rules", "")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ─── create / list / get ──────────────────────────────────────────────────────

#[tokio::test]
async fn create_and_list_and_get() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login_as(&app, TEST_USER).await;

    let id = create_pct_rule(&app, &session, &csrf).await;

    let (status, body_str) = send(&app, authed_get("/accounts/acc1/rules", &session)).await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    let list: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["name"], "commissione");

    let (status, body_str) = send(
        &app,
        authed_get(&format!("/accounts/acc1/rules/{id}"), &session),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    let got: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(got["id"], id);
    assert_eq!(got["calc_type"], "pct_avg_balance");
    assert_eq!(got["value"], 100);
    assert_eq!(got["cadence"], "monthly");
}

#[tokio::test]
async fn get_missing_returns_404() {
    let (app, _, _) = build_test_router().await;
    let (session, _) = login_as(&app, TEST_USER).await;
    let (status, _) = send(&app, authed_get("/accounts/acc1/rules/ghost", &session)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ─── update / delete ──────────────────────────────────────────────────────────

#[tokio::test]
async fn update_rule_value() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login_as(&app, TEST_USER).await;
    let id = create_pct_rule(&app, &session, &csrf).await;

    let body = serde_json::to_vec(&serde_json::json!({ "value": 250 })).unwrap();
    let (status, body_str) = send(
        &app,
        authed_put(&format!("/accounts/acc1/rules/{id}"), &session, &csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    let got: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(got["value"], 250);
    // Untouched fields are preserved.
    assert_eq!(got["calc_type"], "pct_avg_balance");
}

#[tokio::test]
async fn delete_rule_then_404() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login_as(&app, TEST_USER).await;
    let id = create_pct_rule(&app, &session, &csrf).await;

    let (status, _) = send(
        &app,
        authed_delete(&format!("/accounts/acc1/rules/{id}"), &session, &csrf),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = send(
        &app,
        authed_get(&format!("/accounts/acc1/rules/{id}"), &session),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ─── validation ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn invalid_charge_day_is_rejected() {
    let (app, _, _) = build_test_router().await;
    let (session, csrf) = login_as(&app, TEST_USER).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "name": "bad",
        "category_id": "cat-fallback-out",
        "calc_type": "fixed",
        "value": 100,
        "cadence": "monthly",
        "charge_day": 32
    }))
    .unwrap();
    let (status, body_str) = send(
        &app,
        authed_post("/accounts/acc1/rules", &session, &csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body_str}");
}

// ─── generate ─────────────────────────────────────────────────────────────────

/// 1% of a constant 50,000.00 base balance is 500.00 (50,000 cents) each month;
/// generating materializes one fee row per month on the 1st.
#[tokio::test]
async fn generate_materializes_fee_rows() {
    let (app, _, storage) = build_test_router().await;
    seed_tx(&storage, "2024-01-01", 5_000_000).await;

    let (session, csrf) = login_as(&app, TEST_USER).await;
    let id = create_pct_rule(&app, &session, &csrf).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "from": "2024-01-01",
        "to": "2024-03-31"
    }))
    .unwrap();
    let (status, body_str) = send(
        &app,
        authed_post(
            &format!("/accounts/acc1/rules/{id}/generate"),
            &session,
            &csrf,
            body,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let rows: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let arr = rows.as_array().unwrap();
    assert_eq!(arr.len(), 3, "expected Jan/Feb/Mar: {body_str}");
    for row in arr {
        assert_eq!(row["cents"], -50_000);
    }
    assert_eq!(arr[0]["date"], "2024-01-01");
    assert_eq!(arr[2]["date"], "2024-03-01");
}

// ─── preview ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn preview_all_active_rules() {
    let (app, _, storage) = build_test_router().await;
    seed_tx(&storage, "2024-01-01", 5_000_000).await;

    let (session, csrf) = login_as(&app, TEST_USER).await;
    create_pct_rule(&app, &session, &csrf).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "from": "2024-01-01",
        "to": "2024-03-31"
    }))
    .unwrap();
    let (status, body_str) = send(
        &app,
        authed_post("/accounts/acc1/rules/preview", &session, &csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let previews: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let arr = previews.as_array().unwrap();
    assert_eq!(arr.len(), 1, "one active rule: {body_str}");
    assert_eq!(arr[0]["name"], "commissione");
    let rows = arr[0]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["cents"], -50_000);
}

// ─── multi-user permission gate ───────────────────────────────────────────────

/// Gate: an unrelated user with no share of the account cannot read its rules.
#[tokio::test]
async fn unshared_user_cannot_read_rules() {
    let (app, _, _) = build_test_router().await;
    let (owner_session, _) = login_as(&app, TEST_USER).await;

    let (other_session, _) = login_as(&app, OTHER_USER).await;
    let (status, _) = send(&app, authed_get("/accounts/acc1/rules", &other_session)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The owner, by contrast, can list the account's rules.
    let (status, _) = send(&app, authed_get("/accounts/acc1/rules", &owner_session)).await;
    assert_eq!(status, StatusCode::OK);
}

/// Gate: an unrelated user cannot create a rule on an account they do not own.
#[tokio::test]
async fn unshared_user_cannot_create_rule() {
    let (app, _, _) = build_test_router().await;
    let (other_session, other_csrf) = login_as(&app, OTHER_USER).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "name": "intruso",
        "category_id": "cat-fallback-out",
        "calc_type": "fixed",
        "value": 100,
        "cadence": "monthly",
        "charge_day": 1
    }))
    .unwrap();
    let (status, _) = send(
        &app,
        authed_post("/accounts/acc1/rules", &other_session, &other_csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
