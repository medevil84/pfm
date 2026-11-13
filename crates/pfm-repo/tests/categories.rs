//! Repository-level CRUD tests for categories on SQLite in-memory.

use pfm_domain::{Category, CategoryKind};
use pfm_repo::{slugify, CategoriesRepo, DbSettings, Engine, SqlxCategoriesRepo, Storage};

async fn setup() -> (Storage, SqlxCategoriesRepo) {
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

    let repo = SqlxCategoriesRepo::new(&storage);
    (storage, repo)
}

fn sample_category(name: &str, kind: CategoryKind) -> Category {
    Category {
        id: String::new(),
        key: String::new(),
        name: name.into(),
        emoji: "🍔".into(),
        kind,
        budget_cents: Some(15000),
        is_fallback: false,
        created_at: String::new(),
        owner_id: None,
    }
}

#[tokio::test]
async fn create_generates_key_and_id() {
    let (_storage, repo) = setup().await;

    let cat = sample_category("Ristoranti", CategoryKind::Out);
    let created = repo.create(&cat).await.expect("create");

    assert!(!created.id.is_empty());
    assert!(!created.created_at.is_empty());
    assert_eq!(created.key, "ristoranti");
    assert_eq!(created.name, "Ristoranti");
    assert_eq!(created.kind, CategoryKind::Out);
    assert!(!created.is_fallback);
}

#[tokio::test]
async fn create_with_special_chars_slugifies() {
    let (_storage, repo) = setup().await;

    let cat = sample_category("Trasporti & Uscite", CategoryKind::Out);
    let created = repo.create(&cat).await.expect("create");

    assert_eq!(created.key, "trasporti-uscite");
}

#[tokio::test]
async fn get_by_id_missing_returns_none() {
    let (_storage, repo) = setup().await;

    let result = repo.get_by_id("nope").await.expect("get");
    assert!(result.is_none());
}

#[tokio::test]
async fn list_includes_seed_fallbacks() {
    let (_storage, repo) = setup().await;

    let list = repo.list().await.expect("list");
    // The migration seeds two fallback categories.
    assert!(
        list.iter()
            .any(|c| c.is_fallback && c.kind == CategoryKind::Out),
        "expected a fallback out category"
    );
    assert!(
        list.iter()
            .any(|c| c.is_fallback && c.kind == CategoryKind::In),
        "expected a fallback in category"
    );
}

#[tokio::test]
async fn update_changes_name_emoji_budget() {
    let (_storage, repo) = setup().await;

    let cat = sample_category("Spesa", CategoryKind::Out);
    let created = repo.create(&cat).await.expect("create");

    let mut updated = created.clone();
    updated.name = "Supermercato".into();
    updated.emoji = "🛒".into();
    updated.budget_cents = Some(50000);

    repo.update(&updated).await.expect("update");

    let fetched = repo
        .get_by_id(&created.id)
        .await
        .expect("get")
        .expect("found");
    assert_eq!(fetched.name, "Supermercato");
    assert_eq!(fetched.emoji, "🛒");
    assert_eq!(fetched.budget_cents, Some(50000));
    // key is immutable
    assert_eq!(fetched.key, "spesa");
}

#[tokio::test]
async fn delete_user_category_succeeds() {
    let (_storage, repo) = setup().await;

    let cat = sample_category("Temp", CategoryKind::Out);
    let created = repo.create(&cat).await.expect("create");

    let deleted = repo.delete(&created.id).await.expect("delete");
    assert!(deleted);

    let after = repo.get_by_id(&created.id).await.expect("get");
    assert!(after.is_none());
}

#[tokio::test]
async fn delete_missing_returns_false() {
    let (_storage, repo) = setup().await;

    let deleted = repo.delete("ghost").await.expect("delete");
    assert!(!deleted);
}

#[test]
fn slugify_various_inputs() {
    assert_eq!(slugify("Hello World"), "hello-world");
    assert_eq!(slugify("  --Multiple   Dashes--  "), "multiple-dashes");
    assert_eq!(slugify("123"), "123");
    assert_eq!(slugify("!!!"), "cat");
    assert_eq!(slugify(""), "cat");
    assert_eq!(slugify("Caffè & Pausa"), "caff-pausa");
}
