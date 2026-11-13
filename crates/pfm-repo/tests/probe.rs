//! Probe: verify the sqlx `Any` driver round-trips the value kinds the auth
//! schema relies on on SQLite:
//!   * date/time stored as ISO-8601 TEXT  -> read back as `String`
//!   * booleans stored as INTEGER 0/1      -> read back as `i64`
//!   * plain strings and VARCHAR keys
//!
//! The `Any` driver does NOT decode `chrono` types, so the repository layer
//! stores temporal values as ISO-8601 strings and converts at the boundary.
//! This probe de-risks that strategy before the auth repositories are built.

use chrono::{DateTime, Utc};

use sqlx::Row;

use pfm_repo::{DbSettings, Engine, Storage};

#[tokio::test]
async fn any_driver_string_datetime_and_int_bool_roundtrip() {
    sqlx::any::install_default_drivers();

    let settings = DbSettings {
        engine: Engine::Sqlite,
        sqlite_path: Some(":memory:".to_string()),
        sqlite_apply_pragmas: true,
        sqlite_max_connections: 1,
        mariadb_url: None,
        mariadb_max_connections: 0,
        mariadb_min_connections: 0,
    };
    let storage = Storage::connect(&settings).await.expect("connect :memory:");
    let migrator = sqlx::migrate!("../../migrations");
    storage.migrate(&migrator).await.expect("migrate");
    let pool = storage.pool();

    let ts: DateTime<Utc> = "2026-09-03T12:34:56Z".parse().expect("parse ts");
    let ts_iso = ts.to_rfc3339(); // e.g. 2026-09-03T12:34:56Z

    // Insert with ISO string timestamps + integer bool.
    sqlx::query(
        "INSERT INTO users \
         (id, username, email, password_hash, role, active, locale, default_currency, last_seen_at, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind("probe-user")
    .bind("probe")
    .bind("probe@example.com")
    .bind("$argon2id$probe")
    .bind("user")
    .bind(1i64)
    .bind("it")
    .bind("EUR")
    .bind(&ts_iso)
    .bind(&ts_iso)
    .execute(pool)
    .await
    .expect("insert user");

    // Read the whole row using only Any-driver-supported kinds: String + i64.
    let row = sqlx::query(
        "SELECT username, email, role, active, locale, default_currency, last_seen_at, created_at \
         FROM users WHERE id = $1",
    )
    .bind("probe-user")
    .fetch_one(pool)
    .await
    .expect("select user");

    let username: String = row.try_get("username").expect("username");
    let email: String = row.try_get("email").expect("email");
    let role: String = row.try_get("role").expect("role");
    let active: i64 = row.try_get("active").expect("active");
    let locale: String = row.try_get("locale").expect("locale");
    let cur: String = row.try_get("default_currency").expect("default_currency");
    let last_seen: Option<String> = row.try_get("last_seen_at").expect("last_seen_at");
    let created: String = row.try_get("created_at").expect("created_at");

    assert_eq!(username, "probe");
    assert_eq!(email, "probe@example.com");
    assert_eq!(role, "user");
    assert_eq!(active, 1, "integer bool round-trip");
    assert_eq!(locale, "it");
    assert_eq!(cur, "EUR");
    assert_eq!(
        last_seen.as_deref(),
        Some(ts_iso.as_str()),
        "nullable ts as String"
    );
    assert_eq!(created, ts_iso, "timestamp as ISO String");

    // Convert ISO string back to DateTime<Utc> at the boundary.
    let parsed: DateTime<Utc> = DateTime::parse_from_rfc3339(&created)
        .expect("parse rfc3339")
        .with_timezone(&Utc);
    assert_eq!(parsed, ts, "String -> DateTime<Utc> conversion");

    eprintln!("probe OK: ISO String + int-bool round-trip on the Any/SQLite driver");
}
