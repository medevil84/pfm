//! HTTP integration tests for the recurrence routes: auth, CRUD, runtime
//! occurrence generation, per-date overrides, and the multi-user permission
//! gate (rules are permission-checked against their account).

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
const OTHER_USER: &str = "otheruser";

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
        .merge(pfm_http::recurrence_routes::recurrence_routes())
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

/// Create a fixed monthly rule on the seeded account and return its id.
async fn create_rule(app: &Router, session: &str, csrf: &str) -> String {
    let body = serde_json::to_vec(&serde_json::json!({
        "description": "affitto",
        "category_id": "cat-fallback-out",
        "kind": "out",
        "amount_cents": 1000,
        "account_id": "acc1",
        "pattern": "FREQ=MONTHLY;BYMONTHDAY=15",
        "weekend": "create",
        "first_date": "2024-01-15",
        "end_type": "never",
        "mode": "fixed",
        "active": true
    }))
    .unwrap();
    let (status, body_str) = send(app, authed_post("/recurrences", session, csrf, body)).await;
    assert_eq!(status, StatusCode::CREATED, "body: {body_str}");
    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    parsed["id"].as_str().unwrap().to_string()
}

// ─── auth ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn recurrences_require_session() {
    let (app, _) = build_test_router().await;
    let (status, _) = send(&app, authed_get("/recurrences", "")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ─── create / list / get ──────────────────────────────────────────────────────

#[tokio::test]
async fn create_and_list_and_get() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login_as(&app, TEST_USER).await;

    let id = create_rule(&app, &session, &csrf).await;

    let (status, body_str) = send(&app, authed_get("/recurrences", &session)).await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    let list: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["pattern"], "FREQ=MONTHLY;BYMONTHDAY=15");

    let (status, body_str) = send(&app, authed_get(&format!("/recurrences/{id}"), &session)).await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    let got: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(got["id"], id);
    assert_eq!(got["amount_cents"], 1000);
    assert_eq!(got["end_type"], "never");
}

#[tokio::test]
async fn get_missing_returns_404() {
    let (app, _) = build_test_router().await;
    let (session, _) = login_as(&app, TEST_USER).await;
    let (status, _) = send(&app, authed_get("/recurrences/ghost", &session)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ─── update / delete ──────────────────────────────────────────────────────────

#[tokio::test]
async fn update_rule_amount() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login_as(&app, TEST_USER).await;
    let id = create_rule(&app, &session, &csrf).await;

    let body = serde_json::to_vec(&serde_json::json!({ "amount_cents": 1250 })).unwrap();
    let (status, body_str) = send(
        &app,
        authed_put(&format!("/recurrences/{id}"), &session, &csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    let got: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(got["amount_cents"], 1250);
    // Immutable fields are preserved.
    assert_eq!(got["kind"], "out");
}

#[tokio::test]
async fn update_rule_realigns_registered_occurrences() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login_as(&app, TEST_USER).await;
    let id = create_rule(&app, &session, &csrf).await;

    let put = |body: serde_json::Value| {
        authed_put(
            &format!("/recurrences/{id}"),
            &session,
            &csrf,
            serde_json::to_vec(&body).unwrap(),
        )
    };

    // Plain update: nothing realigned, counters present and zero.
    let (status, body_str) = send(&app, put(serde_json::json!({ "amount_cents": 1100 }))).await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    let got: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(
        (got["aligned"].as_u64(), got["skipped_manual"].as_u64()),
        (Some(0), Some(0))
    );

    // With the flag: the registered rows (created at 1000) are rewritten.
    let (status, body_str) = send(
        &app,
        put(serde_json::json!({
            "amount_cents": 1200,
            "apply_to_existing": true,
            "apply_from": "2025-01-01",
            "overwrite_manual": true
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    let got: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(got["amount_cents"], 1200);
    assert!(got["aligned"].as_u64().unwrap() >= 12, "body: {body_str}");
    assert_eq!(got["skipped_manual"], 0);

    // A malformed cut-off date is a client error.
    let (status, _) = send(
        &app,
        put(serde_json::json!({ "apply_to_existing": true, "apply_from": "nope" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn delete_rule_then_404() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login_as(&app, TEST_USER).await;
    let id = create_rule(&app, &session, &csrf).await;

    let (status, _) = send(
        &app,
        authed_delete(&format!("/recurrences/{id}"), &session, &csrf),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = send(&app, authed_get(&format!("/recurrences/{id}"), &session)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ─── occurrences ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn occurrences_generated_at_runtime() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login_as(&app, TEST_USER).await;
    let id = create_rule(&app, &session, &csrf).await;

    let uri = format!("/recurrences/{id}/occurrences?from=2024-01-01&to=2024-03-31");
    let (status, body_str) = send(&app, authed_get(&uri, &session)).await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");

    let occs: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let arr = occs.as_array().unwrap();
    assert_eq!(arr.len(), 3, "expected Jan/Feb/Mar: {body_str}");
    assert_eq!(arr[0]["date"], "2024-01-15");
    assert_eq!(arr[0]["amount_cents"], 1000);
    assert_eq!(arr[0]["overridden"], false);
}

// ─── overrides ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn set_override_pins_date_and_rebases_rule() {
    let (app, _) = build_test_router().await;
    let (session, csrf) = login_as(&app, TEST_USER).await;
    let id = create_rule(&app, &session, &csrf).await;

    // Confirm February with a real amount and re-base the fixed rule.
    let body = serde_json::to_vec(&serde_json::json!({
        "amount_cents": 1500,
        "description": "feb reale",
        "update_rule": true
    }))
    .unwrap();
    let (status, body_str) = send(
        &app,
        authed_put(
            &format!("/recurrences/{id}/overrides/2024-02-15"),
            &session,
            &csrf,
            body,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    let ov: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(ov["date"], "2024-02-15");
    assert_eq!(ov["amount_cents"], 1500);

    // The rule amount is re-based to the confirmed value.
    let (status, body_str) = send(&app, authed_get(&format!("/recurrences/{id}"), &session)).await;
    assert_eq!(status, StatusCode::OK);
    let rec: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(rec["amount_cents"], 1500);

    // Occurrences: Feb is the confirmed/overridden value; Mar adopts the re-based
    // rule amount; Jan stays an estimate at the re-based amount too.
    let uri = format!("/recurrences/{id}/occurrences?from=2024-01-01&to=2024-03-31");
    let (status, body_str) = send(&app, authed_get(&uri, &session)).await;
    assert_eq!(status, StatusCode::OK, "body: {body_str}");
    let occs: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let arr = occs.as_array().unwrap();
    let feb = arr.iter().find(|o| o["date"] == "2024-02-15").unwrap();
    let mar = arr.iter().find(|o| o["date"] == "2024-03-15").unwrap();
    assert_eq!(feb["amount_cents"], 1500);
    assert_eq!(feb["overridden"], true);
    assert_eq!(mar["amount_cents"], 1500);
    assert_eq!(mar["overridden"], false);
}

// ─── multi-user permission gate ───────────────────────────────────────────────

/// Gate: a rule is permission-checked against its account; an unrelated user
/// who has no share of that account cannot read it.
#[tokio::test]
async fn unshared_user_cannot_read_rule() {
    let (app, _) = build_test_router().await;
    let (owner_session, owner_csrf) = login_as(&app, TEST_USER).await;
    let id = create_rule(&app, &owner_session, &owner_csrf).await;

    let (other_session, _) = login_as(&app, OTHER_USER).await;
    let (status, _) = send(
        &app,
        authed_get(&format!("/recurrences/{id}"), &other_session),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// Gate: an unrelated user cannot override a rule they cannot access.
#[tokio::test]
async fn unshared_user_cannot_override_rule() {
    let (app, _) = build_test_router().await;
    let (owner_session, owner_csrf) = login_as(&app, TEST_USER).await;
    let id = create_rule(&app, &owner_session, &owner_csrf).await;

    let (other_session, other_csrf) = login_as(&app, OTHER_USER).await;
    let body = serde_json::to_vec(&serde_json::json!({ "amount_cents": 999 })).unwrap();
    let (status, _) = send(
        &app,
        authed_put(
            &format!("/recurrences/{id}/overrides/2024-02-15"),
            &other_session,
            &other_csrf,
            body,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
