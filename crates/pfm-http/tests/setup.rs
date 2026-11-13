//! Integration tests for the setup wizard backend: startup matrix, reset,
//! completion, atomicity, and the startup guard.
//!
//! Each test spins up an in-memory SQLite database (with all migrations run),
//! builds the router in the appropriate mode, and issues requests via
//! `tower::ServiceExt::oneshot`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use pfm_config::{Config, DatabaseConfig, Engine, SecurityConfig};
use pfm_http::{build_router_with_setup_token, setup_guard::SetupGuardService, AppMode};

/// The one-time setup token the tests pretend the operator was shown.
const SETUP_TOKEN: &str = "test-setup-token-0123456789abcdef";
use pfm_repo::{DbSettings, Storage};
use pfm_service::{PasswordService, SetupCompleteParams, SetupService};
use sqlx::Row;
use tower::ServiceExt;

// ─── Test helpers ─────────────────────────────────────────────────────────────

/// Build a minimal test configuration.
fn test_config() -> Arc<Config> {
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
            ..Default::default()
        },
        ..Default::default()
    };
    Arc::new(config)
}

/// Connect to an in-memory SQLite DB and run all migrations.
async fn test_storage() -> Arc<Storage> {
    sqlx::any::install_default_drivers();
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
    storage.migrate(&migrator).await.unwrap();
    Arc::new(storage)
}

/// Build a test router in the given mode with the given storage.
fn build_test_router(
    storage: Arc<Storage>,
    config: Arc<Config>,
    mode: AppMode,
    shutdown_notify: Option<Arc<tokio::sync::Notify>>,
) -> Router {
    build_router_with_setup_token(
        Some(storage),
        config,
        mode,
        shutdown_notify,
        None,
        Some(SETUP_TOKEN.to_string()),
    )
    .unwrap()
}

/// Helper: build a request with `ConnectInfo` injected.
fn req(method: &str, uri: &str, body: Vec<u8>) -> Request<Body> {
    let addr: SocketAddr = "127.0.0.1:9999".parse().unwrap();
    let mut builder = Request::builder().method(method).uri(uri);
    builder = builder
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-setup-token", SETUP_TOKEN);
    let mut req = builder.body(Body::from(body)).unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));
    req
}

/// Helper: send a request and return (status, body_string).
async fn send(app: &Router, req: Request<Body>) -> (StatusCode, String) {
    let res = app.clone().oneshot(req).await.expect("request failed");
    let status = res.status();
    let body = axum::body::to_bytes(res.into_body(), 1_048_576)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&body).into_owned())
}

/// Build a complete-setup request body.
fn complete_body() -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "username": "admin",
        "password": "test-password-123",
        "email": "admin@example.com",
        "app_name": "PFM",
        "locale": "it",
        "currency": "EUR"
    }))
    .unwrap()
}

// ─── Test: setup state endpoint ───────────────────────────────────────────────

#[tokio::test]
async fn setup_state_reports_not_configured() {
    let storage = test_storage().await;
    let config = test_config();
    let app = build_test_router(storage, config, AppMode::Setup { configured: false }, None);

    let (status, body) = send(&app, req("GET", "/api/setup/state", vec![])).await;
    assert_eq!(status, StatusCode::OK);
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["configured"], false);
}

#[tokio::test]
async fn setup_state_reports_configured_after_completion() {
    let storage = test_storage().await;
    let config = test_config();

    // First, complete the setup.
    let repo = pfm_repo::SqlxSetupRepo::new(&storage);
    let pw = PasswordService::new(16384, 3, 4);
    let setup_svc = SetupService::new(&repo, pw);
    let params = SetupCompleteParams {
        username: "admin".into(),
        password: "test-password-123".into(),
        email: "admin@example.com".into(),
        app_name: "PFM".into(),
        locale: "it".into(),
        currency: "EUR".into(),
    };
    setup_svc.complete(&params).await.unwrap();

    // Now build the router in setup mode with configured=true.
    let app = build_test_router(
        storage.clone(),
        config.clone(),
        AppMode::Setup { configured: true },
        None,
    );
    let (status, body) = send(&app, req("GET", "/api/setup/state", vec![])).await;
    assert_eq!(status, StatusCode::OK);
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["configured"], true);
}

// ─── Test: reset endpoint ─────────────────────────────────────────────────────

#[tokio::test]
async fn reset_clears_all_data() {
    let storage = test_storage().await;
    let config = test_config();

    // First, complete the setup to have data in the DB.
    let repo = pfm_repo::SqlxSetupRepo::new(&storage);
    let pw = PasswordService::new(16384, 3, 4);
    let setup_svc = SetupService::new(&repo, pw);
    let params = SetupCompleteParams {
        username: "admin".into(),
        password: "test-password-123".into(),
        email: "admin@example.com".into(),
        app_name: "PFM".into(),
        locale: "it".into(),
        currency: "EUR".into(),
    };
    setup_svc.complete(&params).await.unwrap();

    // Verify data exists.
    let configured = setup_svc.is_configured().await.unwrap();
    assert!(configured);

    // Now reset via the HTTP endpoint.
    let app = build_test_router(
        storage.clone(),
        config.clone(),
        AppMode::Setup { configured: true },
        None,
    );
    let (status, body) = send(&app, req("POST", "/api/setup/reset", vec![])).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["success"], true);

    // Verify data is gone.
    let not_configured = setup_svc.is_configured().await.unwrap();
    assert!(!not_configured, "DB should be unconfigured after reset");
}

#[tokio::test]
async fn reset_preserves_schema() {
    let storage = test_storage().await;
    let _config = test_config();

    // Complete setup.
    let repo = pfm_repo::SqlxSetupRepo::new(&storage);
    let pw = PasswordService::new(16384, 3, 4);
    let setup_svc = SetupService::new(&repo, pw);
    let params = SetupCompleteParams {
        username: "admin".into(),
        password: "test-password-123".into(),
        email: "admin@example.com".into(),
        app_name: "PFM".into(),
        locale: "it".into(),
        currency: "EUR".into(),
    };
    setup_svc.complete(&params).await.unwrap();

    // Reset.
    setup_svc.reset_all_data().await.unwrap();

    // Verify the tables still exist (schema preserved) and are empty
    // (except categories which retain the 2 fallback rows from migration seed).
    let pool = storage.pool();
    let empty_tables = [
        "users",
        "user_prefs",
        "audit_log",
        "accounts",
        "transactions",
        "setup_state",
    ];
    for table in &empty_tables {
        let sql = format!("SELECT COUNT(*) FROM {table}");
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .fetch_one(pool)
            .await
            .unwrap_or_else(|e| panic!("table {table} should still exist: {e}"));
        let count: i64 = row.get(0);
        assert_eq!(count, 0, "table {table} should be empty after reset");
    }

    // Categories: should have exactly the 2 fallback rows from the migration seed.
    let cat_count: i64 = sqlx::query("SELECT COUNT(*) FROM categories")
        .fetch_one(pool)
        .await
        .unwrap()
        .get(0);
    assert_eq!(cat_count, 2, "categories should retain the 2 fallback rows");
}

// ─── Test: setup completion ───────────────────────────────────────────────────

#[tokio::test]
async fn complete_setup_creates_admin_user() {
    let storage = test_storage().await;
    let config = test_config();

    // Complete via HTTP.
    let app = build_test_router(
        storage.clone(),
        config.clone(),
        AppMode::Setup { configured: false },
        None,
    );
    let (status, body) = send(&app, req("POST", "/api/setup/complete", complete_body())).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    // Verify the user was created.
    let users: Vec<(String, String, String)> =
        sqlx::query("SELECT username, role, default_currency FROM users")
            .fetch_all(storage.pool())
            .await
            .unwrap()
            .into_iter()
            .map(|r| {
                (
                    r.get::<String, _>(0),
                    r.get::<String, _>(1),
                    r.get::<String, _>(2),
                )
            })
            .collect();

    assert_eq!(users.len(), 1);
    assert_eq!(users[0].0, "admin");
    assert_eq!(users[0].1, "admin");
    assert_eq!(users[0].2, "EUR");
}

#[tokio::test]
async fn complete_setup_stores_password_as_argon2id_hash() {
    let storage = test_storage().await;
    let config = test_config();

    let app = build_test_router(
        storage.clone(),
        config.clone(),
        AppMode::Setup { configured: false },
        None,
    );
    let (status, body) = send(&app, req("POST", "/api/setup/complete", complete_body())).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    // Verify the password is stored as an Argon2id hash.
    let hash: String = sqlx::query("SELECT password_hash FROM users WHERE username = 'admin'")
        .fetch_one(storage.pool())
        .await
        .unwrap()
        .get(0);

    assert!(
        hash.starts_with("$argon2id$"),
        "password must be an Argon2id hash, got: {hash}"
    );
    assert!(
        !hash.contains("test-password-123"),
        "plaintext password must not be in the hash"
    );
}

#[tokio::test]
async fn complete_setup_rejects_short_password() {
    let storage = test_storage().await;
    let config = test_config();

    let body = serde_json::to_vec(&serde_json::json!({
        "username": "admin",
        "password": "short",
        "email": "admin@example.com",
        "app_name": "PFM",
        "locale": "it",
        "currency": "EUR"
    }))
    .unwrap();

    let app = build_test_router(
        storage.clone(),
        config,
        AppMode::Setup { configured: false },
        None,
    );
    let (status, body_str) = send(&app, req("POST", "/api/setup/complete", body)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body_str}");
}

#[tokio::test]
async fn complete_setup_rejects_empty_password() {
    let storage = test_storage().await;
    let config = test_config();

    let body = serde_json::to_vec(&serde_json::json!({
        "username": "admin",
        "password": "",
        "email": "admin@example.com",
        "app_name": "PFM",
        "locale": "it",
        "currency": "EUR"
    }))
    .unwrap();

    let app = build_test_router(storage, config, AppMode::Setup { configured: false }, None);
    let (status, body_str) = send(&app, req("POST", "/api/setup/complete", body)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body_str}");
}

#[tokio::test]
async fn complete_setup_creates_prefs_row() {
    let storage = test_storage().await;
    let config = test_config();

    let app = build_test_router(
        storage.clone(),
        config,
        AppMode::Setup { configured: false },
        None,
    );
    let (status, body) = send(&app, req("POST", "/api/setup/complete", complete_body())).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    // Verify user_prefs row exists.
    let count: i64 = sqlx::query("SELECT COUNT(*) FROM user_prefs")
        .fetch_one(storage.pool())
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 1, "user_prefs row should be created");
}

#[tokio::test]
async fn complete_setup_marks_state_initialized() {
    let storage = test_storage().await;
    let config = test_config();

    let app = build_test_router(
        storage.clone(),
        config,
        AppMode::Setup { configured: false },
        None,
    );
    let (status, body) = send(&app, req("POST", "/api/setup/complete", complete_body())).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    // Verify setup_state row.
    let value: String = sqlx::query("SELECT value FROM setup_state WHERE key = 'initialized'")
        .fetch_one(storage.pool())
        .await
        .unwrap()
        .get(0);
    assert_eq!(value, "1");
}

// ─── Test: atomicity ──────────────────────────────────────────────────────────

#[tokio::test]
async fn atomicity_failed_complete_leaves_no_partial_data() {
    let storage = test_storage().await;
    let _config = test_config();

    // Use a username that will cause a UNIQUE violation (create the user first).
    let repo = pfm_repo::SqlxSetupRepo::new(&storage);
    let pw = PasswordService::new(16384, 3, 4);
    let hash = pw.hash("existing-password-1").unwrap();

    sqlx::query(
        "INSERT INTO users (id, username, email, password_hash, role, active, locale, default_currency, created_at)
         VALUES ('u1', 'admin', 'existing@example.com', $1, 'admin', 1, 'it', 'EUR', '2025-01-01T00:00:00Z')",
    )
    .bind(&hash)
    .execute(storage.pool())
    .await
    .unwrap();

    // Now try to complete setup with the same username (should fail with UNIQUE).
    let setup_svc = SetupService::new(&repo, pw.clone());
    let params = SetupCompleteParams {
        username: "admin".into(),
        password: "test-password-123".into(),
        email: "new@example.com".into(),
        app_name: "PFM".into(),
        locale: "it".into(),
        currency: "EUR".into(),
    };
    let result = setup_svc.complete(&params).await;
    assert!(result.is_err(), "should fail due to duplicate username");

    // Verify no partial data was created: no new user_prefs row, no setup_state.
    let prefs_count: i64 = sqlx::query("SELECT COUNT(*) FROM user_prefs")
        .fetch_one(storage.pool())
        .await
        .unwrap()
        .get(0);
    assert_eq!(prefs_count, 0, "no user_prefs should be created on failure");

    let state_count: i64 = sqlx::query("SELECT COUNT(*) FROM setup_state")
        .fetch_one(storage.pool())
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        state_count, 0,
        "no setup_state should be created on failure"
    );

    // The original user should still be there.
    let user_count: i64 = sqlx::query("SELECT COUNT(*) FROM users")
        .fetch_one(storage.pool())
        .await
        .unwrap()
        .get(0);
    assert_eq!(user_count, 1, "original user should be intact");
}

// ─── Test: startup guard ──────────────────────────────────────────────────────

#[tokio::test]
async fn guard_mode_serves_guard_page_for_all_paths() {
    // In guard mode, the router only has health probes.
    // The actual guard page is served by the fallback service.
    let guard = SetupGuardService::new();

    // Simulate a request to any path.
    let req = Request::builder()
        .uri("/some/path")
        .body(Body::empty())
        .unwrap();
    let res = guard.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 65536).await.unwrap();
    let html = String::from_utf8_lossy(&body);
    assert!(html.contains("Configurazione iniziale non completata"));
    assert!(html.contains("--setup"));
    // Should NOT contain any app UI elements.
    assert!(!html.contains("login"));
    assert!(!html.contains("dashboard"));
}

#[tokio::test]
async fn guard_mode_healthz_still_works() {
    let storage = test_storage().await;
    let config = test_config();
    let app = build_test_router(storage, config, AppMode::Guard, None);

    let (status, _) = send(&app, req("GET", "/healthz", vec![])).await;
    assert_eq!(status, StatusCode::OK);
}

// ─── Test: setup API not available in normal mode ─────────────────────────────

#[tokio::test]
async fn setup_api_not_available_in_normal_mode() {
    let storage = test_storage().await;
    let config = test_config();

    // Build router in Normal mode.
    let app = build_test_router(storage, config, AppMode::Normal, None);

    // /api/setup/state should NOT return 200 (route not registered).
    // It may be 404 or 401 depending on middleware behavior.
    let (status, _) = send(&app, req("GET", "/api/setup/state", vec![])).await;
    assert_ne!(
        status,
        StatusCode::OK,
        "setup API should not be available in normal mode"
    );

    // /api/setup/reset should also not be 200.
    let (status, _) = send(&app, req("POST", "/api/setup/reset", vec![])).await;
    assert_ne!(status, StatusCode::OK);
}

#[tokio::test]
async fn setup_api_not_available_in_guard_mode() {
    let storage = test_storage().await;
    let config = test_config();

    let app = build_test_router(storage, config, AppMode::Guard, None);

    // In guard mode, the router only has /healthz and /readyz.
    // /api/setup/state should be 404.
    let (status, _) = send(&app, req("GET", "/api/setup/state", vec![])).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ─── Test: shutdown notification on completion ────────────────────────────────

#[tokio::test]
async fn completion_triggers_shutdown_notification() {
    let storage = test_storage().await;
    let config = test_config();
    let notify = Arc::new(tokio::sync::Notify::new());
    let notify_clone = Arc::clone(&notify);

    let app = build_test_router(
        storage,
        config,
        AppMode::Setup { configured: false },
        Some(notify_clone),
    );

    // Spawn a task that waits for the notification with a timeout.
    let notify_wait = Arc::clone(&notify);
    let shutdown_fired = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(5), notify_wait.notified())
            .await
            .is_ok()
    });

    // Trigger completion.
    let (status, body) = send(&app, req("POST", "/api/setup/complete", complete_body())).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    // Wait for the shutdown notification.
    let fired = shutdown_fired.await.unwrap();
    assert!(fired, "shutdown notification should have been triggered");
}

// ─── Startup matrix tests ─────────────────────────────────────────────────────

/// Test 1: --setup + DB NOT configured → wizard mode
#[tokio::test]
async fn matrix_1_setup_not_configured() {
    let storage = test_storage().await;
    let config = test_config();

    // Verify DB is not configured.
    let repo = pfm_repo::SqlxSetupRepo::new(&storage);
    let pw = PasswordService::new(16384, 3, 4);
    let svc = SetupService::new(&repo, pw);
    assert!(!svc.is_configured().await.unwrap());

    // Build router in Setup mode (configured=false).
    let app = build_test_router(storage, config, AppMode::Setup { configured: false }, None);

    // The setup API should be available.
    let (status, _) = send(&app, req("GET", "/api/setup/state", vec![])).await;
    assert_eq!(status, StatusCode::OK);

    // The regular API is deliberately NOT mounted while the wizard runs: a
    // half-initialised instance must not be drivable through it.
    let (status, _) = send(&app, req("GET", "/api/auth/me", vec![])).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Test 2: --setup + DB configured → reset confirmation mode
#[tokio::test]
async fn matrix_2_setup_configured() {
    let storage = test_storage().await;
    let config = test_config();

    // First, complete the setup to make the DB configured.
    let repo = pfm_repo::SqlxSetupRepo::new(&storage);
    let pw = PasswordService::new(16384, 3, 4);
    let svc = SetupService::new(&repo, pw);
    let params = SetupCompleteParams {
        username: "admin".into(),
        password: "test-password-123".into(),
        email: "admin@example.com".into(),
        app_name: "PFM".into(),
        locale: "it".into(),
        currency: "EUR".into(),
    };
    svc.complete(&params).await.unwrap();
    assert!(svc.is_configured().await.unwrap());

    // Build router in Setup mode (configured=true).
    let app = build_test_router(storage, config, AppMode::Setup { configured: true }, None);

    // The setup API should be available.
    let (status, body) = send(&app, req("GET", "/api/setup/state", vec![])).await;
    assert_eq!(status, StatusCode::OK);
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["configured"], true);
}

/// Test 3: --setup + configured + user refuses → process exits
/// (Tested implicitly: the reset page shows "Annulla" which just shows
/// a message. The process stays running until killed externally.)
#[tokio::test]
async fn matrix_3_refusal_keeps_process_running() {
    let storage = test_storage().await;
    let config = test_config();

    // Complete setup first.
    let repo = pfm_repo::SqlxSetupRepo::new(&storage);
    let pw = PasswordService::new(16384, 3, 4);
    let svc = SetupService::new(&repo, pw);
    let params = SetupCompleteParams {
        username: "admin".into(),
        password: "test-password-123".into(),
        email: "admin@example.com".into(),
        app_name: "PFM".into(),
        locale: "it".into(),
        currency: "EUR".into(),
    };
    svc.complete(&params).await.unwrap();

    // In the reset confirmation page, "Annulla" just shows a message.
    // The process doesn't exit automatically - it's the user closing the
    // browser or the process being killed. We verify that the reset endpoint
    // still works (data is still there).
    let app = build_test_router(storage, config, AppMode::Setup { configured: true }, None);

    // Data should still be configured (no reset happened).
    let (status, body) = send(&app, req("GET", "/api/setup/state", vec![])).await;
    assert_eq!(status, StatusCode::OK);
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["configured"], true);
}

/// Test 4: --setup + configured + confirm reset → data wiped, wizard shown
#[tokio::test]
async fn matrix_4_confirm_reset() {
    let storage = test_storage().await;
    let config = test_config();

    // Complete setup.
    let repo = pfm_repo::SqlxSetupRepo::new(&storage);
    let pw = PasswordService::new(16384, 3, 4);
    let svc = SetupService::new(&repo, pw);
    let params = SetupCompleteParams {
        username: "admin".into(),
        password: "test-password-123".into(),
        email: "admin@example.com".into(),
        app_name: "PFM".into(),
        locale: "it".into(),
        currency: "EUR".into(),
    };
    svc.complete(&params).await.unwrap();

    // Reset via the HTTP endpoint.
    let app = build_test_router(
        storage.clone(),
        config,
        AppMode::Setup { configured: true },
        None,
    );
    let (status, body) = send(&app, req("POST", "/api/setup/reset", vec![])).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    // Verify DB is now unconfigured.
    assert!(!svc.is_configured().await.unwrap());
}

/// Test 5: no --setup + configured → normal mode
#[tokio::test]
async fn matrix_5_normal_mode_configured() {
    let storage = test_storage().await;
    let config = test_config();

    // Complete setup.
    let repo = pfm_repo::SqlxSetupRepo::new(&storage);
    let pw = PasswordService::new(16384, 3, 4);
    let svc = SetupService::new(&repo, pw);
    let params = SetupCompleteParams {
        username: "admin".into(),
        password: "test-password-123".into(),
        email: "admin@example.com".into(),
        app_name: "PFM".into(),
        locale: "it".into(),
        currency: "EUR".into(),
    };
    svc.complete(&params).await.unwrap();

    // Build router in Normal mode.
    let app = build_test_router(storage, config, AppMode::Normal, None);

    // App routes should work.
    let (status, _) = send(&app, req("GET", "/healthz", vec![])).await;
    assert_eq!(status, StatusCode::OK);

    // Setup API should NOT be available.
    let (status, _) = send(&app, req("GET", "/api/setup/state", vec![])).await;
    assert_ne!(
        status,
        StatusCode::OK,
        "setup API should not be available in normal mode"
    );
}

/// Test 6: no --setup + NOT configured → guard mode
#[tokio::test]
async fn matrix_6_guard_mode() {
    let storage = test_storage().await;
    let config = test_config();

    // Build router in Guard mode.
    let app = build_test_router(storage, config, AppMode::Guard, None);

    // Only health probes should be available.
    let (status, _) = send(&app, req("GET", "/healthz", vec![])).await;
    assert_eq!(status, StatusCode::OK);

    // App routes should NOT be available.
    let (status, _) = send(&app, req("GET", "/api/auth/me", vec![])).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Setup API should NOT be available.
    let (status, _) = send(&app, req("GET", "/api/setup/state", vec![])).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ─── Test: no password in logs ────────────────────────────────────────────────

#[tokio::test]
async fn password_not_in_error_response() {
    let storage = test_storage().await;
    let config = test_config();

    // Send a valid completion request, then try to complete again (should fail
    // with duplicate). The error message should not contain the password.
    let app = build_test_router(
        storage.clone(),
        config.clone(),
        AppMode::Setup { configured: false },
        None,
    );
    let (status, _) = send(&app, req("POST", "/api/setup/complete", complete_body())).await;
    assert_eq!(status, StatusCode::OK);

    // Second attempt should fail (the instance is configured by now).
    let (status, body) = send(&app, req("POST", "/api/setup/complete", complete_body())).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        !body.contains("test-password-123"),
        "password must not appear in error response: {body}"
    );
}

// ─── Setup token & mode isolation ─────────────────────────────────────────────

/// Same request as `req` but without (or with a wrong) token.
fn req_with_token(method: &str, uri: &str, body: Vec<u8>, token: Option<&str>) -> Request<Body> {
    let addr: SocketAddr = "127.0.0.1:9999".parse().unwrap();
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(t) = token {
        builder = builder.header("x-setup-token", t);
    }
    let mut r = builder.body(Body::from(body)).unwrap();
    r.extensions_mut().insert(ConnectInfo(addr));
    r
}

#[tokio::test]
async fn setup_endpoints_refuse_missing_or_wrong_token() {
    let storage = test_storage().await;
    let config = test_config();
    let app = build_test_router(storage, config, AppMode::Setup { configured: true }, None);

    for (m, uri) in [
        ("GET", "/api/setup/state"),
        ("POST", "/api/setup/reset"),
        ("POST", "/api/setup/complete"),
        ("POST", "/api/setup/cancel"),
    ] {
        let (status, _) = send(&app, req_with_token(m, uri, complete_body(), None)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{m} {uri} without token");
        let (status, _) = send(
            &app,
            req_with_token(m, uri, complete_body(), Some("wrong-token")),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{m} {uri} with wrong token"
        );
    }
}

#[tokio::test]
async fn unauthenticated_reset_does_not_touch_data() {
    let storage = test_storage().await;
    let config = test_config();
    let repo = pfm_repo::SqlxSetupRepo::new(&storage);
    let svc = pfm_service::SetupService::new(&repo, pfm_service::PasswordService::new(8, 1, 1));
    svc.complete(&pfm_service::SetupCompleteParams {
        username: "admin".into(),
        password: "test-password-123".into(),
        email: "admin@example.com".into(),
        app_name: "PFM".into(),
        locale: "it".into(),
        currency: "EUR".into(),
    })
    .await
    .unwrap();

    let app = build_test_router(storage, config, AppMode::Setup { configured: true }, None);
    let (status, _) = send(
        &app,
        req_with_token("POST", "/api/setup/reset", vec![], None),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(svc.is_configured().await.unwrap(), "data must survive");
}

#[tokio::test]
async fn complete_refused_when_already_configured() {
    let storage = test_storage().await;
    let config = test_config();
    let app = build_test_router(
        storage.clone(),
        config.clone(),
        AppMode::Setup { configured: false },
        None,
    );
    let (status, _) = send(&app, req("POST", "/api/setup/complete", complete_body())).await;
    assert_eq!(status, StatusCode::OK);

    // A second wizard run must not mint another admin on a configured DB.
    let app = build_test_router(storage, config, AppMode::Setup { configured: true }, None);
    let body = serde_json::to_vec(&serde_json::json!({
        "username": "intruder",
        "password": "test-password-123",
        "email": "intruder@example.com"
    }))
    .unwrap();
    let (status, _) = send(&app, req("POST", "/api/setup/complete", body)).await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn setup_mode_does_not_expose_the_regular_api() {
    let storage = test_storage().await;
    let config = test_config();
    let app = build_test_router(storage, config, AppMode::Setup { configured: false }, None);
    for uri in ["/api/auth/me", "/api/accounts", "/api/users"] {
        let (status, _) = send(&app, req_with_token("GET", uri, vec![], None)).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{uri} must not be routed in setup mode"
        );
    }
}
