//! HTTP integration tests for the import routes.
//!
//! Covers:
//! - Euristica "Data operazione;Data valuta;Descrizione;Importo" (content-based)
//! - Idempotenza: doppio import non produce doppioni
//! - Preview non scrive nel DB
//! - Tre formati data (dmy, ymd, mdy) e separatori decimali

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
const TEST_PASS: &str = "correct-horse-battery-staple";

fn addr() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 12345))
}

const UNICREDIT_CSV: &str = "Data operazione;Data valuta;Descrizione;Importo\n\
    15/01/2024;15/01/2024;Supermercato SPA;-45,50\n\
    16/01/2024;16/01/2024;Stipendio;+2500,00\n\
    17/01/2024;17/01/2024;Farmacia;-12,30";

const YMD_CSV: &str = "Date,Description,Amount\n\
    2024-01-15,Grocery Store,-12.34\n\
    2024-01-16,Salary,5000.00";

const MDY_CSV: &str = "Date,Description,Amount\n\
    01/15/2024,Coffee Shop,4.50\n\
    01/16/2024,Restaurant,32.75";

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
        "INSERT INTO accounts (id, owner_id, name, kind, icon, currency, start_date, start_balance_cents, active, created_at)
         VALUES ('acc1', 'u1', 'Conto Corrente', 'current', '', 'EUR', '2020-01-01', 0, 1, '2025-01-01T00:00:00Z')",
    )
    .execute(pool)
    .await
    .unwrap();

    let storage = Arc::new(storage);
    let state = Arc::new(AppState {
        auth: Arc::new(AuthService::new(&storage, pw)),
        prefs: Arc::new(PrefsService::new(&storage)),
        accounts: Arc::new(AccountService::new(&storage)),
        categories: Arc::new(CategoryService::new(&storage)),
        sharing: Arc::new(SharingService::new(&storage)),
        transactions: Arc::new(TransactionService::new(&storage)),
        transfers: Arc::new(TransferService::new(&storage)),
        cards: Arc::new(CardService::new(&storage)),
        deposits: Arc::new(DepositService::new(&storage)),
        bulk: Arc::new(BulkService::new(&storage)),
        tags: Arc::new(pfm_service::TagService::new(&storage)),
        holidays: Arc::new(pfm_service::HolidayService::new(&storage)),
        recurrences: Arc::new(RecurrenceService::new(&storage)),
        rules: Arc::new(FeeRuleService::new(&storage)),
        virtuals: Arc::new(VirtualAccountService::new(&storage)),
        stats: Arc::new(StatsService::new(&storage)),
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
        rate_limiter: Arc::new(pfm_http::rate_limit::RateLimiter::new(
            100,
            Duration::from_secs(60),
        )),
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
        .merge(pfm_http::auth_routes::auth_routes(state.clone()))
        .merge(pfm_http::import_routes::import_routes())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            csrf_middleware,
        ))
        .layer(session_layer)
        .with_state(state.clone());

    (app, state, pool_owned)
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
    let body =
        serde_json::to_vec(&serde_json::json!({"username": TEST_USER, "password": TEST_PASS}))
            .unwrap();
    let res = app
        .clone()
        .oneshot(req_with("POST", "/auth/login", body))
        .await
        .unwrap();
    let status = res.status();
    assert_eq!(status, StatusCode::OK, "login should succeed");

    let cookies = res
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect::<Vec<_>>();
    let session_cookie = cookies
        .iter()
        .find(|c| c.contains("session"))
        .cloned()
        .unwrap_or_default();

    let full = axum::body::to_bytes(res.into_body(), 1_048_576)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&full).unwrap();
    let csrf = json["csrf_token"].as_str().unwrap().to_string();

    (session_cookie, csrf)
}

fn authed_post(uri: &str, session: &str, csrf: &str, body: Vec<u8>) -> Request<Body> {
    let mut builder = Request::builder().method("POST").uri(uri);
    builder = builder
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, session)
        .header("x-csrf-token", csrf);
    let mut req = builder.body(Body::from(body)).unwrap();
    req.extensions_mut().insert(ConnectInfo(addr()));
    req
}

fn authed_get(uri: &str, session: &str) -> Request<Body> {
    let mut builder = Request::builder().method("GET").uri(uri);
    builder = builder.header(header::COOKIE, session);
    let mut req = builder.body(Body::empty()).unwrap();
    req.extensions_mut().insert(ConnectInfo(addr()));
    req
}

async fn count_transactions(pool: &sqlx::AnyPool, account_id: &str) -> i64 {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions WHERE account_id = $1")
        .bind(account_id)
        .fetch_one(pool)
        .await
        .unwrap();
    count
}

async fn count_profiles(pool: &sqlx::AnyPool) -> i64 {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM import_profiles")
        .fetch_one(pool)
        .await
        .unwrap();
    count
}

// ─── Test: Euristica "Data operazione" ────────────────────────────────────────

#[tokio::test]
async fn preview_unicredit_heuristic_picks_correct_columns() {
    let (router, _state, _pool) = build_test_router().await;
    let (cookie, csrf) = login(&router).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "account_id": "acc1",
        "bank_name": "Unicredit",
        "csv_content": UNICREDIT_CSV,
        "delimiter": ";",
        "date_format": "dmy",
        "decimal_sep": ",",
    }))
    .unwrap();

    let (status, body_str) = send(
        &router,
        authed_post("/import/preview", &cookie, &csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body_str).unwrap();

    assert_eq!(json["total_rows"], 3);
    assert_eq!(json["to_import"].as_array().unwrap().len(), 3);
    assert_eq!(json["duplicates"].as_array().unwrap().len(), 0);

    let to_import = json["to_import"].as_array().unwrap();
    assert_eq!(to_import[0]["date"], "2024-01-15");
    assert_eq!(to_import[0]["cents"], -4550);
    assert_eq!(to_import[0]["description"], "Supermercato SPA");

    assert_eq!(to_import[1]["date"], "2024-01-16");
    assert_eq!(to_import[1]["cents"], 250000);
    assert_eq!(to_import[1]["description"], "Stipendio");

    assert_eq!(to_import[2]["date"], "2024-01-17");
    assert_eq!(to_import[2]["cents"], -1230);
    assert_eq!(to_import[2]["description"], "Farmacia");
}

// ─── Test: Preview non scrive nel DB ─────────────────────────────────────────

#[tokio::test]
async fn preview_does_not_write_to_db() {
    let (router, _state, pool) = build_test_router().await;
    let (cookie, csrf) = login(&router).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "account_id": "acc1",
        "bank_name": "Unicredit",
        "csv_content": UNICREDIT_CSV,
        "delimiter": ";",
        "date_format": "dmy",
        "decimal_sep": ",",
    }))
    .unwrap();

    for _ in 0..3 {
        let (status, _) = send(
            &router,
            authed_post("/import/preview", &cookie, &csrf, body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    assert_eq!(count_transactions(&pool, "acc1").await, 0);
    assert_eq!(count_profiles(&pool).await, 0);
}

// ─── Test: Idempotenza (doppio import) ───────────────────────────────────────

#[tokio::test]
async fn double_import_is_idempotent() {
    let (router, _state, pool) = build_test_router().await;
    let (cookie, csrf) = login(&router).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "account_id": "acc1",
        "bank_name": "Unicredit",
        "csv_content": UNICREDIT_CSV,
        "delimiter": ";",
        "date_format": "dmy",
        "decimal_sep": ",",
    }))
    .unwrap();

    // First commit.
    let (status, body_str) = send(
        &router,
        authed_post("/import/commit", &cookie, &csrf, body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(json["inserted"], 3);
    assert_eq!(count_transactions(&pool, "acc1").await, 3);

    // Second commit (same file).
    let (status, body_str) =
        send(&router, authed_post("/import/commit", &cookie, &csrf, body)).await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(json["inserted"], 0);
    assert_eq!(json["skipped"], 3);

    assert_eq!(count_transactions(&pool, "acc1").await, 3);
}

// ─── Test: Tre formati data + separatori decimali ────────────────────────────

#[tokio::test]
async fn import_ymd_format_dot_separator() {
    let (router, _state, pool) = build_test_router().await;
    let (cookie, csrf) = login(&router).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "account_id": "acc1",
        "bank_name": "US Bank",
        "csv_content": YMD_CSV,
        "delimiter": ",",
        "date_format": "ymd",
        "decimal_sep": ".",
    }))
    .unwrap();

    let (status, body_str) =
        send(&router, authed_post("/import/commit", &cookie, &csrf, body)).await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(json["inserted"], 2);

    let rows: Vec<(String, i64, Option<String>)> = sqlx::query_as(
        "SELECT date, cents, description FROM transactions WHERE account_id = 'acc1' ORDER BY date",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, "2024-01-15");
    assert_eq!(rows[0].1, -1234);
    assert_eq!(rows[0].2.as_deref(), Some("Grocery Store"));

    assert_eq!(rows[1].0, "2024-01-16");
    assert_eq!(rows[1].1, 500000);
    assert_eq!(rows[1].2.as_deref(), Some("Salary"));
}

#[tokio::test]
async fn import_mdy_format_dot_separator() {
    let (router, _state, pool) = build_test_router().await;
    let (cookie, csrf) = login(&router).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "account_id": "acc1",
        "bank_name": "US Bank MDY",
        "csv_content": MDY_CSV,
        "delimiter": ",",
        "date_format": "mdy",
        "decimal_sep": ".",
    }))
    .unwrap();

    let (status, body_str) =
        send(&router, authed_post("/import/commit", &cookie, &csrf, body)).await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(json["inserted"], 2);

    let rows: Vec<(String, i64, Option<String>)> = sqlx::query_as(
        "SELECT date, cents, description FROM transactions WHERE account_id = 'acc1' ORDER BY date",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, "2024-01-15");
    assert_eq!(rows[0].1, 450);
    assert_eq!(rows[0].2.as_deref(), Some("Coffee Shop"));

    assert_eq!(rows[1].0, "2024-01-16");
    assert_eq!(rows[1].1, 3275);
    assert_eq!(rows[1].2.as_deref(), Some("Restaurant"));
}

#[tokio::test]
async fn import_dmy_format_comma_separator() {
    let (router, _state, pool) = build_test_router().await;
    let (cookie, csrf) = login(&router).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "account_id": "acc1",
        "bank_name": "IT Bank",
        "csv_content": UNICREDIT_CSV,
        "delimiter": ";",
        "date_format": "dmy",
        "decimal_sep": ",",
    }))
    .unwrap();

    let (status, body_str) =
        send(&router, authed_post("/import/commit", &cookie, &csrf, body)).await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(json["inserted"], 3);

    let rows: Vec<(String, i64, Option<String>)> = sqlx::query_as(
        "SELECT date, cents, description FROM transactions WHERE account_id = 'acc1' ORDER BY date",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].0, "2024-01-15");
    assert_eq!(rows[0].1, -4550);
    assert_eq!(rows[0].2.as_deref(), Some("Supermercato SPA"));
    assert_eq!(rows[1].0, "2024-01-16");
    assert_eq!(rows[1].1, 250000);
    assert_eq!(rows[2].0, "2024-01-17");
    assert_eq!(rows[2].1, -1230);
}

// ─── Test: Profilo salvato dopo commit ────────────────────────────────────────

#[tokio::test]
async fn commit_saves_profile_for_bank() {
    let (router, _state, pool) = build_test_router().await;
    let (cookie, csrf) = login(&router).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "account_id": "acc1",
        "bank_name": "Unicredit",
        "csv_content": UNICREDIT_CSV,
        "delimiter": ";",
        "date_format": "dmy",
        "decimal_sep": ",",
    }))
    .unwrap();

    let (status, _) = send(&router, authed_post("/import/commit", &cookie, &csrf, body)).await;
    assert_eq!(status, StatusCode::OK);

    assert_eq!(count_profiles(&pool).await, 1);

    let profile: (String, String, String) = sqlx::query_as(
        "SELECT delimiter, date_format, decimal_sep FROM import_profiles WHERE bank_name = 'Unicredit'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(profile.0, ";");
    assert_eq!(profile.1, "dmy");
    assert_eq!(profile.2, ",");
}

// ─── Test: GET /import/profiles ──────────────────────────────────────────────

#[tokio::test]
async fn list_profiles_empty_then_populated() {
    let (router, _state, _pool) = build_test_router().await;
    let (cookie, csrf) = login(&router).await;

    // Initially no profiles.
    let (status, body_str) = send(&router, authed_get("/import/profiles", &cookie)).await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(json["profiles"].as_array().unwrap().len(), 0);

    // Commit an import to create a profile.
    let body = serde_json::to_vec(&serde_json::json!({
        "account_id": "acc1",
        "bank_name": "Unicredit",
        "csv_content": UNICREDIT_CSV,
        "delimiter": ";",
        "date_format": "dmy",
        "decimal_sep": ",",
    }))
    .unwrap();
    let (status, _) = send(&router, authed_post("/import/commit", &cookie, &csrf, body)).await;
    assert_eq!(status, StatusCode::OK);

    // Now there should be 1 profile.
    let (status, body_str) = send(&router, authed_get("/import/profiles", &cookie)).await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(json["profiles"].as_array().unwrap().len(), 1);
    assert_eq!(json["profiles"][0]["bank_name"], "Unicredit");
    assert_eq!(json["profiles"][0]["delimiter"], ";");
    assert_eq!(json["profiles"][0]["date_format"], "dmy");
    assert_eq!(json["profiles"][0]["decimal_sep"], ",");
}

// ─── Test: Preview con dati già importati mostra already_imported ─────────────

#[tokio::test]
async fn preview_after_commit_shows_already_imported() {
    let (router, _state, _pool) = build_test_router().await;
    let (cookie, csrf) = login(&router).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "account_id": "acc1",
        "bank_name": "Unicredit",
        "csv_content": UNICREDIT_CSV,
        "delimiter": ";",
        "date_format": "dmy",
        "decimal_sep": ",",
    }))
    .unwrap();

    // Commit first.
    let (status, _) = send(
        &router,
        authed_post("/import/commit", &cookie, &csrf, body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Now preview the same file: all rows should be "already_imported".
    let (status, body_str) = send(
        &router,
        authed_post("/import/preview", &cookie, &csrf, body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body_str).unwrap();

    assert_eq!(json["total_rows"], 3);
    assert_eq!(json["already_imported"].as_array().unwrap().len(), 3);
    assert_eq!(json["to_import"].as_array().unwrap().len(), 0);
    assert_eq!(json["duplicates"].as_array().unwrap().len(), 0);
}
