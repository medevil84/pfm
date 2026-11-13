//! Repository-level CRUD tests for accounts on SQLite in-memory.

use chrono::NaiveDate;
use pfm_domain::{Account, AccountKind};
use pfm_repo::{AccountsRepo, DbSettings, Engine, SqlxAccountsRepo, Storage};

async fn setup() -> (Storage, SqlxAccountsRepo) {
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
    let storage = Storage::connect(&settings).await.expect("connect");
    let migrator = sqlx::migrate!("../../migrations");
    storage.migrate(&migrator).await.expect("migrate");

    // Insert a test user for the FK.
    let pool = storage.pool();
    sqlx::query(
        "INSERT INTO users \
         (id, username, email, password_hash, role, active, locale, default_currency, last_seen_at, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind("test-user")
    .bind("testuser")
    .bind("test@example.com")
    .bind("$argon2id$test")
    .bind("user")
    .bind(1i64)
    .bind("it")
    .bind("EUR")
    .bind("2026-01-01T00:00:00Z")
    .bind("2026-01-01T00:00:00Z")
    .execute(pool)
    .await
    .expect("insert user");

    let repo = SqlxAccountsRepo::new(&storage);
    (storage, repo)
}

fn sample_account(name: &str, kind: AccountKind) -> Account {
    Account {
        id: String::new(),
        owner_id: "test-user".into(),
        name: name.into(),
        kind,
        icon: "bi-bank".into(),
        note: None,
        iban: Some("IT60X0542811101000000123456".into()),
        currency: "EUR".into(),
        start_date: NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(),
        start_balance_cents: 100000,
        limit_cents: None,
        charge_day: None,
        closing_day: None,
        primary_account_id: None,
        rate_pct_bps: None,
        goal_cents: None,
        min_balance_cents: None,
        active: true,
        created_at: String::new(),
        sort_order: 0,
        last_settled_date: NaiveDate::from_ymd_opt(1900, 1, 1).unwrap(),
    }
}

#[tokio::test]
async fn create_and_get_by_id() {
    let (_storage, repo) = setup().await;

    let acc = sample_account("Corrente", AccountKind::Current);
    let created = repo.create("test-user", &acc).await.expect("create");

    assert!(!created.id.is_empty());
    assert!(!created.created_at.is_empty());
    assert_eq!(created.owner_id, "test-user");
    assert_eq!(created.name, "Corrente");
    assert_eq!(created.kind, AccountKind::Current);

    let fetched = repo
        .get_by_id(&created.id)
        .await
        .expect("get_by_id")
        .expect("found");
    assert_eq!(fetched.id, created.id);
    assert_eq!(
        fetched.start_date,
        NaiveDate::from_ymd_opt(2024, 1, 15).unwrap()
    );
    assert_eq!(fetched.start_balance_cents, 100000);
    assert!(fetched.active);
}

#[tokio::test]
async fn get_by_id_missing_returns_none() {
    let (_storage, repo) = setup().await;

    let result = repo.get_by_id("nonexistent-id").await.expect("get_by_id");
    assert!(result.is_none());
}

#[tokio::test]
async fn list_by_owner() {
    let (_storage, repo) = setup().await;

    let a = sample_account("A", AccountKind::Current);
    let b = sample_account("B", AccountKind::Card);
    repo.create("test-user", &a).await.expect("create a");
    repo.create("test-user", &b).await.expect("create b");

    let list = repo.list_by_owner("test-user").await.expect("list");
    assert_eq!(list.len(), 2);
}

#[tokio::test]
async fn update_changes_mutable_fields() {
    let (_storage, repo) = setup().await;

    let acc = sample_account("Before", AccountKind::Current);
    let created = repo.create("test-user", &acc).await.expect("create");

    let mut updated = created.clone();
    updated.name = "After".into();
    updated.active = false;

    repo.update(&updated).await.expect("update");

    let fetched = repo
        .get_by_id(&created.id)
        .await
        .expect("get")
        .expect("found");
    assert_eq!(fetched.name, "After");
    assert!(!fetched.active);
}

#[tokio::test]
async fn delete_removes_row() {
    let (_storage, repo) = setup().await;

    let acc = sample_account("Temp", AccountKind::Virtual);
    let created = repo.create("test-user", &acc).await.expect("create");

    let deleted = repo.delete(&created.id).await.expect("delete");
    assert!(deleted);

    let after = repo.get_by_id(&created.id).await.expect("get");
    assert!(after.is_none());
}

#[tokio::test]
async fn delete_missing_returns_false() {
    let (_storage, repo) = setup().await;

    let deleted = repo.delete("ghost-id").await.expect("delete");
    assert!(!deleted);
}

#[tokio::test]
async fn referenced_as_primary_detects_backing() {
    let (_storage, repo) = setup().await;

    let current = sample_account("Primary", AccountKind::Current);
    let primary = repo
        .create("test-user", &current)
        .await
        .expect("create primary");

    let mut deposit = sample_account("Deposito", AccountKind::Deposit);
    deposit.primary_account_id = Some(primary.id.clone());
    deposit.rate_pct_bps = Some(300);
    repo.create("test-user", &deposit)
        .await
        .expect("create deposit");

    let referenced = repo
        .referenced_as_primary(&primary.id)
        .await
        .expect("check ref");
    assert!(referenced);

    let unreferenced = repo
        .referenced_as_primary("no-such-id")
        .await
        .expect("check no ref");
    assert!(!unreferenced);
}

#[tokio::test]
async fn card_account_fields_roundtrip() {
    let (_storage, repo) = setup().await;

    let mut acc = sample_account("Mastercard", AccountKind::Card);
    acc.limit_cents = Some(500000);
    acc.charge_day = Some(25);

    let created = repo.create("test-user", &acc).await.expect("create");

    let fetched = repo
        .get_by_id(&created.id)
        .await
        .expect("get")
        .expect("found");
    assert_eq!(fetched.kind, AccountKind::Card);
    assert_eq!(fetched.limit_cents, Some(500000));
    assert_eq!(fetched.charge_day, Some(25));
}
