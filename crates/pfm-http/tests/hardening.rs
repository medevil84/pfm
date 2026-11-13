//! Regression tests for the security audit findings.
//!
//! Each test drives the fully wired router (`build_router`: session layer,
//! session guard, CSRF, security headers) over an in-memory SQLite database, the
//! way a browser would — cookies and `X-CSRF-Token` included — and asserts that
//! the exploit that used to work is now refused.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use pfm_config::{Argon2Config, Config, DatabaseConfig, Engine, RateLimitConfig, SecurityConfig};
use pfm_http::{build_router, AppMode};
use pfm_repo::{DbSettings, Storage};
use pfm_service::PasswordService;
use serde_json::{json, Value};
use tower::ServiceExt;

const PASS: &str = "correct-horse-battery-staple";

fn config(per_min: u32) -> Arc<Config> {
    Arc::new(Config {
        database: DatabaseConfig {
            engine: Engine::Sqlite,
            sqlite_path: Some(":memory:".into()),
            ..Default::default()
        },
        security: SecurityConfig {
            session_secret: Some(
                "test-secret-key-test-secret-key-test-secret-key-0123456789".into(),
            ),
            argon2: Argon2Config {
                memory_kib: 64,
                iterations: 1,
                parallelism: 1,
            },
            rate_limit_login: RateLimitConfig {
                per_min,
                window_s: 60,
            },
            ..Default::default()
        },
        scheduler: pfm_config::SchedulerConfig {
            enabled: false,
            interval_secs: 3600,
        },
        ..Default::default()
    })
}

struct World {
    app: Router,
    storage: Arc<Storage>,
}

async fn world_with(config: Arc<Config>) -> World {
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
    storage
        .migrate(&sqlx::migrate!("../../migrations"))
        .await
        .unwrap();
    let storage = Arc::new(storage);
    let app = build_router(Some(storage.clone()), config, AppMode::Normal, None, None).unwrap();
    World { app, storage }
}

async fn world() -> World {
    world_with(config(1000)).await
}

impl World {
    async fn add_user(&self, id: &str, username: &str, role: &str) {
        let hash = PasswordService::new(64, 1, 1).hash(PASS).unwrap();
        sqlx::query(
            "INSERT INTO users (id, username, email, password_hash, role, active, locale, \
             default_currency, created_at) VALUES ($1, $2, $3, $4, $5, 1, 'en', 'EUR', \
             '2025-01-01T00:00:00Z')",
        )
        .bind(id)
        .bind(username)
        .bind(format!("{username}@example.com"))
        .bind(hash)
        .bind(role)
        .execute(self.storage.pool())
        .await
        .unwrap();
    }

    fn client(&self) -> Client {
        Client {
            app: self.app.clone(),
            cookies: HashMap::new(),
            csrf: String::new(),
            port: 40000,
        }
    }

    async fn login(&self, username: &str) -> Client {
        let mut c = self.client();
        let (status, body) = c
            .send(
                "POST",
                "/api/auth/login",
                Some(json!({"username": username, "password": PASS})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "login {username}: {body}");
        c
    }
}

struct Client {
    app: Router,
    cookies: HashMap<String, String>,
    csrf: String,
    /// Source port of the next request: a new TCP connection each time.
    port: u16,
}

impl Client {
    async fn raw(
        &mut self,
        method: &str,
        uri: &str,
        content_type: Option<&str>,
        body: Vec<u8>,
        extra: &[(&str, &str)],
    ) -> (StatusCode, axum::http::HeaderMap, String) {
        self.port = self.port.wrapping_add(1).max(1024);
        let addr: SocketAddr = format!("203.0.113.9:{}", self.port).parse().unwrap();
        let mut b = Request::builder().method(method).uri(uri);
        if let Some(ct) = content_type {
            b = b.header(header::CONTENT_TYPE, ct);
        }
        if !self.cookies.is_empty() {
            let cookie = self
                .cookies
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("; ");
            b = b.header(header::COOKIE, cookie);
        }
        if !self.csrf.is_empty() {
            b = b.header("x-csrf-token", &self.csrf);
        }
        for (k, v) in extra {
            b = b.header(*k, *v);
        }
        let mut req = b.body(Body::from(body)).unwrap();
        req.extensions_mut().insert(ConnectInfo(addr));
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        for v in headers.get_all(header::SET_COOKIE) {
            let s = v.to_str().unwrap();
            let first = s.split(';').next().unwrap();
            if let Some((k, val)) = first.split_once('=') {
                if val.is_empty() {
                    self.cookies.remove(k);
                } else {
                    self.cookies.insert(k.to_string(), val.to_string());
                }
            }
        }
        let bytes = axum::body::to_bytes(res.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap();
        (
            status,
            headers,
            String::from_utf8_lossy(&bytes).into_owned(),
        )
    }

    async fn send(&mut self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, String) {
        let bytes = body
            .map(|b| serde_json::to_vec(&b).unwrap())
            .unwrap_or_default();
        let (status, _, text) = self
            .raw(method, uri, Some("application/json"), bytes, &[])
            .await;
        if uri.ends_with("/auth/login") && status == StatusCode::OK {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                self.csrf = v["csrf_token"].as_str().unwrap_or("").to_string();
            }
        }
        (status, text)
    }

    async fn json(&mut self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let (s, t) = self.send(method, uri, body).await;
        (s, serde_json::from_str(&t).unwrap_or(Value::Null))
    }

    async fn account(&mut self, name: &str, kind: &str, extra: Value) -> (StatusCode, Value) {
        let mut body = json!({
            "name": name, "kind": kind, "icon": "bi-bank",
            "start_date": "2026-01-01", "start_balance_cents": 100000
        });
        if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                b.insert(k.clone(), v.clone());
            }
        }
        self.json("POST", "/api/accounts", Some(body)).await
    }

    async fn simple_account(&mut self, name: &str) -> String {
        let (s, v) = self.account(name, "current", json!({})).await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
        v["id"].as_str().unwrap().to_string()
    }

    async fn share(&mut self, account: &str, email: &str, level: &str) {
        let (s, v) = self
            .json(
                "POST",
                &format!("/api/accounts/{account}/shares"),
                Some(json!({"email": email, "level": level})),
            )
            .await;
        assert!(s.is_success(), "share: {v}");
    }
}

// ─── Login: throttling, content type, timing/enumeration ──────────────────────

/// The limiter used to key on `ip:port:username`, so every new TCP connection
/// got a fresh budget.
#[tokio::test]
async fn login_throttle_does_not_depend_on_the_source_port() {
    let w = world_with(config(3)).await;
    w.add_user("u1", "alice", "user").await;
    let mut c = w.client();
    let mut statuses = vec![];
    for i in 0..8 {
        let (s, _) = c
            .send(
                "POST",
                "/api/auth/login",
                Some(json!({"username": "alice", "password": format!("wrong-{i}-password")})),
            )
            .await; // each call uses a different source port
        statuses.push(s);
    }
    assert!(
        statuses.contains(&StatusCode::TOO_MANY_REQUESTS),
        "expected a 429 despite changing ports, got {statuses:?}"
    );
}

/// Spraying one password over many usernames from one address is capped too.
#[tokio::test]
async fn login_throttle_caps_one_address_across_usernames() {
    let w = world_with(config(2)).await; // per-address budget = 12
    let mut c = w.client();
    let mut got_429 = false;
    for i in 0..20 {
        let (s, _) = c
            .send(
                "POST",
                "/api/auth/login",
                Some(json!({"username": format!("user{i}"), "password": "spray-password"})),
            )
            .await;
        got_429 |= s == StatusCode::TOO_MANY_REQUESTS;
    }
    assert!(
        got_429,
        "password spraying across usernames must hit the per-IP cap"
    );
}

#[tokio::test]
async fn login_rejects_non_json_content_type() {
    let w = world().await;
    w.add_user("u1", "alice", "user").await;
    let mut c = w.client();
    let body = serde_json::to_vec(&json!({"username": "alice", "password": PASS})).unwrap();
    for ct in [
        "text/plain",
        "application/x-www-form-urlencoded",
        "multipart/form-data",
    ] {
        let (s, _, _) = c
            .raw("POST", "/api/auth/login", Some(ct), body.clone(), &[])
            .await;
        assert_eq!(s, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{ct}");
    }
    let (s, _, _) = c.raw("POST", "/api/auth/login", None, body, &[]).await;
    assert_eq!(s, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

/// A disabled account used to answer 403 "disabled" to anyone, password or not.
#[tokio::test]
async fn disabled_account_is_indistinguishable_without_the_password() {
    let w = world().await;
    w.add_user("u1", "alice", "user").await;
    sqlx::query("UPDATE users SET active = 0 WHERE id = 'u1'")
        .execute(w.storage.pool())
        .await
        .unwrap();
    let mut c = w.client();
    let (wrong, body_wrong) = c
        .send(
            "POST",
            "/api/auth/login",
            Some(json!({"username": "alice", "password": "nope-nope-nope"})),
        )
        .await;
    let (unknown, body_unknown) = c
        .send(
            "POST",
            "/api/auth/login",
            Some(json!({"username": "nobody", "password": "nope-nope-nope"})),
        )
        .await;
    assert_eq!(wrong, StatusCode::UNAUTHORIZED);
    assert_eq!(wrong, unknown);
    let code = |b: &str| serde_json::from_str::<Value>(b).unwrap()["code"].clone();
    assert_eq!(code(&body_wrong), code(&body_unknown));
    // Only the real owner of the password learns that the account is disabled.
    let (s, _) = c
        .send(
            "POST",
            "/api/auth/login",
            Some(json!({"username": "alice", "password": PASS})),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

// ─── Sessions are revalidated on every request ────────────────────────────────

#[tokio::test]
async fn disabled_user_session_stops_working() {
    let w = world().await;
    w.add_user("adm", "admin", "admin").await;
    w.add_user("u1", "alice", "user").await;
    let mut alice = w.login("alice").await;
    assert_eq!(
        alice.send("GET", "/api/auth/me", None).await.0,
        StatusCode::OK
    );

    let mut admin = w.login("admin").await;
    let (s, _) = admin
        .send("PUT", "/api/users/u1", Some(json!({"active": false})))
        .await;
    assert_eq!(s, StatusCode::OK);

    assert_eq!(
        alice.send("GET", "/api/auth/me", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        alice.send("GET", "/api/accounts", None).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn deleted_user_session_stops_working() {
    let w = world().await;
    w.add_user("adm", "admin", "admin").await;
    w.add_user("u1", "alice", "user").await;
    let mut alice = w.login("alice").await;
    let mut admin = w.login("admin").await;
    let (s, _) = admin.send("DELETE", "/api/users/u1", Some(json!({}))).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(
        alice.send("GET", "/api/auth/me", None).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn password_change_ends_other_sessions_but_keeps_the_current_one() {
    let w = world().await;
    w.add_user("u1", "alice", "user").await;
    let mut laptop = w.login("alice").await;
    let mut stolen = w.login("alice").await;

    let (s, v) = laptop
        .send(
            "PUT",
            "/api/auth/password",
            Some(json!({"current_password": PASS, "new_password": "a-brand-new-passphrase-1"})),
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{v}");

    assert_eq!(
        laptop.send("GET", "/api/auth/me", None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        stolen.send("GET", "/api/auth/me", None).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn admin_password_reset_ends_the_targets_sessions() {
    let w = world().await;
    w.add_user("adm", "admin", "admin").await;
    w.add_user("u1", "alice", "user").await;
    let mut alice = w.login("alice").await;
    let mut admin = w.login("admin").await;
    let (s, _) = admin
        .send(
            "PUT",
            "/api/users/u1",
            Some(json!({"password": "reset-by-admin-123"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        alice.send("GET", "/api/auth/me", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    // The admin resetting their *own* password keeps their session.
    let (s, _) = admin
        .send(
            "PUT",
            "/api/users/adm",
            Some(json!({"password": "my-new-admin-pass-1"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        admin.send("GET", "/api/auth/me", None).await.0,
        StatusCode::OK
    );
}

// ─── Authorization on client-supplied ids ─────────────────────────────────────

/// A card/deposit settles onto its `primary_account_id`: pointing it at
/// somebody else's account used to let the system post rows there.
#[tokio::test]
async fn cannot_back_a_card_with_someone_elses_account() {
    let w = world().await;
    w.add_user("u1", "victim", "user").await;
    w.add_user("u2", "attacker", "user").await;
    let mut victim = w.login("victim").await;
    let mut attacker = w.login("attacker").await;
    let victim_acct = victim.simple_account("Victim current").await;
    let own = attacker.simple_account("Mine").await;

    let (s, v) = attacker
        .account(
            "Evil card",
            "card",
            json!({"limit_cents": 100000, "charge_day": 10, "closing_day": 5,
                   "primary_account_id": victim_acct}),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");

    // Same through an update of an existing account.
    let (s, v) = attacker
        .account(
            "Good card",
            "card",
            json!({"limit_cents": 100000, "charge_day": 10, "closing_day": 5, "primary_account_id": own}),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let card = v["id"].as_str().unwrap();
    let (s, _) = attacker
        .json(
            "PUT",
            &format!("/api/accounts/{card}"),
            Some(json!({"primary_account_id": victim_acct})),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // A read-only share is not enough either: settlement *writes*.
    victim
        .share(&victim_acct, "attacker@example.com", "read")
        .await;
    let (s, _) = attacker
        .json(
            "PUT",
            &format!("/api/accounts/{card}"),
            Some(json!({"primary_account_id": victim_acct})),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    victim
        .share(&victim_acct, "attacker@example.com", "write")
        .await;
    let (s, v) = attacker
        .json(
            "PUT",
            &format!("/api/accounts/{card}"),
            Some(json!({"primary_account_id": victim_acct})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
}

#[tokio::test]
async fn read_only_share_cannot_hijack_a_transfer() {
    let w = world().await;
    w.add_user("u1", "owner", "user").await;
    w.add_user("u2", "reader", "user").await;
    let mut owner = w.login("owner").await;
    let mut reader = w.login("reader").await;
    let (a, b) = (
        owner.simple_account("A").await,
        owner.simple_account("B").await,
    );
    let (r1, r2) = (
        reader.simple_account("R1").await,
        reader.simple_account("R2").await,
    );
    owner.share(&a, "reader@example.com", "read").await;
    owner.share(&b, "reader@example.com", "read").await;
    let (s, v) = owner
        .json(
            "POST",
            "/api/transfers",
            Some(json!({"from_account_id": a, "to_account_id": b, "date": "2026-02-01", "cents": 5000})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let pair = v["pair_id"].as_str().unwrap().to_string();

    let (s, _) = reader
        .json(
            "PUT",
            &format!("/api/transfers/{pair}"),
            Some(json!({"from_account_id": r1, "to_account_id": r2})),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // And the owner's transfer is still where it was.
    let (s, v) = owner
        .json("GET", &format!("/api/transfers/{pair}"), None)
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["from_account_id"], a.as_str());
}

#[tokio::test]
async fn stranger_cannot_move_someone_elses_recurrence() {
    let w = world().await;
    w.add_user("u1", "owner", "user").await;
    w.add_user("u2", "stranger", "user").await;
    let mut owner = w.login("owner").await;
    let mut stranger = w.login("stranger").await;
    let acct = owner.simple_account("Owner account").await;
    let mine = stranger.simple_account("Stranger account").await;

    let (s, v) = owner
        .json(
            "POST",
            "/api/recurrences",
            Some(
                json!({"description": "rent", "category_id": "cat-fallback-out", "kind": "out",
                        "amount_cents": -80000, "account_id": acct,
                        "pattern": "FREQ=MONTHLY;BYMONTHDAY=1", "first_date": "2099-03-01"}),
            ),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let rec = v["id"].as_str().unwrap().to_string();

    let (s, _) = stranger
        .json(
            "PUT",
            &format!("/api/recurrences/{rec}"),
            Some(json!({"account_id": mine})),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // A read-only collaborator cannot pull it out either.
    owner.share(&acct, "stranger@example.com", "read").await;
    let (s, _) = stranger
        .json(
            "PUT",
            &format!("/api/recurrences/{rec}"),
            Some(json!({"account_id": mine})),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, v) = owner
        .json("GET", &format!("/api/recurrences/{rec}"), None)
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["account_id"], acct.as_str());

    // A transfer rule may not link an account its author cannot write to.
    let (s, _) = stranger
        .json(
            "POST",
            "/api/recurrences",
            Some(
                json!({"description": "drain", "category_id": "cat-fallback-out", "kind": "out",
                        "amount_cents": -1000, "account_id": mine, "linked_account_id": acct,
                        "pattern": "FREQ=MONTHLY;BYMONTHDAY=1", "first_date": "2099-03-01"}),
            ),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn import_profiles_list_only_shows_visible_accounts() {
    let w = world().await;
    w.add_user("u1", "owner", "user").await;
    w.add_user("u2", "other", "user").await;
    let mut owner = w.login("owner").await;
    let mut other = w.login("other").await;
    let acct = owner.simple_account("Owner account").await;
    sqlx::query(
        "INSERT INTO import_profiles (id, account_id, bank_name, delimiter, date_format, \
         decimal_sep, auto_cat, skip_dup, created_at) VALUES ('p1', $1, 'SecretBank', ';', \
         'dmy', ',', 0, 0, '2026-01-01T00:00:00Z')",
    )
    .bind(&acct)
    .execute(w.storage.pool())
    .await
    .expect("seed profile");

    let (s, body) = owner.send("GET", "/api/import/profiles", None).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert!(
        body.contains("SecretBank"),
        "the owner must see their own profile: {body}"
    );

    let (s, body) = other.send("GET", "/api/import/profiles", None).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert!(
        !body.contains("SecretBank"),
        "foreign profile leaked: {body}"
    );
}

// ─── Categories and tags belong to their creator ──────────────────────────────

#[tokio::test]
async fn categories_are_private_to_their_owner_and_collaborators() {
    let w = world().await;
    w.add_user("adm", "admin", "admin").await;
    w.add_user("u1", "alice", "user").await;
    w.add_user("u2", "bob", "user").await;
    let mut alice = w.login("alice").await;
    let mut bob = w.login("bob").await;
    let mut admin = w.login("admin").await;

    let (s, v) = alice
        .json(
            "POST",
            "/api/categories",
            Some(json!({"name": "Groceries", "emoji": "🛒", "kind": "out"})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let cat = v["id"].as_str().unwrap().to_string();
    assert_eq!(v["editable"], true);

    // Bob does not even see it, and cannot rename / delete / use it.
    let (_, list) = bob.json("GET", "/api/categories", None).await;
    assert!(list
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["id"] != cat.as_str()));
    let (s, _) = bob
        .json(
            "PUT",
            &format!("/api/categories/{cat}"),
            Some(json!({"name": "HACKED"})),
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = bob
        .json("DELETE", &format!("/api/categories/{cat}"), None)
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let acct = bob.simple_account("Bob account").await;
    let (s, _) = bob
        .json(
            "POST",
            "/api/transactions",
            Some(json!({"account_id": acct, "date": "2026-02-01", "cents": -100, "category_id": cat})),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    // Once they collaborate Bob sees it (read), but still cannot change it.
    let shared = alice_account(&mut alice).await;
    alice.share(&shared, "bob@example.com", "read").await;
    let (_, list) = bob.json("GET", "/api/categories", None).await;
    let row = list
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == cat.as_str())
        .expect("collaborator sees the category");
    assert_eq!(row["editable"], false);
    let (s, _) = bob
        .json(
            "PUT",
            &format!("/api/categories/{cat}"),
            Some(json!({"name": "HACKED"})),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Shared (ownerless) fallbacks are admin-managed; an admin can manage any.
    let (_, list) = bob.json("GET", "/api/categories", None).await;
    let fb = list
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "cat-fallback-out")
        .unwrap();
    assert_eq!(fb["editable"], false);
    let (s, _) = admin
        .json(
            "PUT",
            &format!("/api/categories/{cat}"),
            Some(json!({"name": "Renamed by admin"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);

    // Two users can each have a category with the same name.
    let (s, v) = bob
        .json(
            "POST",
            "/api/categories",
            Some(json!({"name": "Groceries", "emoji": "🥕", "kind": "out"})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    // …but not the same user twice.
    let (s, _) = bob
        .json(
            "POST",
            "/api/categories",
            Some(json!({"name": "Groceries", "emoji": "🥕", "kind": "out"})),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
}

async fn alice_account(c: &mut Client) -> String {
    c.simple_account("Alice shared").await
}

#[tokio::test]
async fn category_in_use_by_a_recurrence_cannot_be_deleted() {
    let w = world().await;
    w.add_user("u1", "alice", "user").await;
    let mut alice = w.login("alice").await;
    let (_, v) = alice
        .json(
            "POST",
            "/api/categories",
            Some(json!({"name": "Rent", "emoji": "🏠", "kind": "out"})),
        )
        .await;
    let cat = v["id"].as_str().unwrap().to_string();
    let acct = alice.simple_account("Main").await;
    let (s, v) = alice
        .json(
            "POST",
            "/api/recurrences",
            Some(
                json!({"description": "rent", "category_id": cat, "kind": "out",
                        "amount_cents": -80000, "account_id": acct,
                        "pattern": "FREQ=MONTHLY;BYMONTHDAY=1", "first_date": "2099-03-01"}),
            ),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let (s, _) = alice
        .json("DELETE", &format!("/api/categories/{cat}"), None)
        .await;
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "deleting would cascade to the recurrence"
    );
}

#[tokio::test]
async fn unknown_category_is_a_validation_error_not_a_500() {
    let w = world().await;
    w.add_user("u1", "alice", "user").await;
    let mut alice = w.login("alice").await;
    let acct = alice.simple_account("Main").await;
    let (s, body) = alice
        .send(
            "POST",
            "/api/transactions",
            Some(json!({"account_id": acct, "date": "2026-02-01", "cents": -5, "category_id": "does-not-exist"})),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(!body.to_lowercase().contains("foreign key"), "{body}");
}

#[tokio::test]
async fn tags_are_per_user_and_only_the_owner_edits_them() {
    let w = world().await;
    w.add_user("u1", "alice", "user").await;
    w.add_user("u2", "bob", "user").await;
    let mut alice = w.login("alice").await;
    let mut bob = w.login("bob").await;

    let (s, v) = alice
        .json("POST", "/api/tags", Some(json!({"label": "holiday"})))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let tag = v["id"].as_str().unwrap().to_string();
    // The same label is free for Bob.
    let (s, v) = bob
        .json("POST", "/api/tags", Some(json!({"label": "holiday"})))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    // Bob cannot see, edit, delete or attach Alice's.
    let (_, list) = bob.json("GET", "/api/tags", None).await;
    assert!(list
        .as_array()
        .unwrap()
        .iter()
        .all(|t| t["id"] != tag.as_str()));
    let (s, _) = bob
        .json(
            "PUT",
            &format!("/api/tags/{tag}"),
            Some(json!({"label": "x"})),
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = bob.json("DELETE", &format!("/api/tags/{tag}"), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let acct = bob.simple_account("Bob").await;
    let (_, tx) = bob
        .json(
            "POST",
            "/api/transactions",
            Some(json!({"account_id": acct, "date": "2026-02-01", "cents": -5})),
        )
        .await;
    let tx_id = tx["id"].as_str().unwrap();
    let (s, _) = bob
        .json(
            "PUT",
            &format!("/api/transactions/{tx_id}/tags"),
            Some(json!({"tag_ids": [tag]})),
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

// ─── Headers, caching, error bodies, CSRF ─────────────────────────────────────

#[tokio::test]
async fn security_headers_cover_framing_and_form_hijacking() {
    let w = world().await;
    let mut c = w.client();
    let (_, headers, _) = c.raw("GET", "/healthz", None, vec![], &[]).await;
    let csp = headers
        .get("content-security-policy")
        .unwrap()
        .to_str()
        .unwrap();
    for d in [
        "frame-ancestors 'none'",
        "form-action 'self'",
        "base-uri 'self'",
        "object-src 'none'",
    ] {
        assert!(csp.contains(d), "CSP lacks {d}: {csp}");
    }
    assert_eq!(headers.get("x-frame-options").unwrap(), "DENY");
}

#[tokio::test]
async fn every_api_response_is_no_store() {
    let w = world().await;
    w.add_user("u1", "alice", "user").await;
    let mut c = w.login("alice").await;
    for path in [
        "/api/tags",
        "/api/cards",
        "/api/holiday-calendars",
        "/api/categories",
        "/api/accounts",
        "/api/anything-new-later",
    ] {
        let (_, headers, _) = c.raw("GET", path, None, vec![], &[]).await;
        let cc = headers.get("cache-control").unwrap().to_str().unwrap();
        assert!(cc.contains("no-store"), "{path}: {cc}");
    }
}

#[tokio::test]
async fn internal_errors_do_not_reach_the_client() {
    use axum::response::IntoResponse;
    let res = pfm_http::error::ApiError::internal(Some(
        "error returned from database: (code: 787) FOREIGN KEY constraint failed".into(),
    ))
    .into_response();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = axum::body::to_bytes(res.into_body(), 64 * 1024)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        !text.contains("FOREIGN KEY") && !text.contains("787"),
        "{text}"
    );
}

#[tokio::test]
async fn csrf_is_enforced_on_every_non_read_method() {
    let w = world().await;
    w.add_user("u1", "alice", "user").await;
    let mut c = w.login("alice").await;
    c.csrf.clear(); // keep the session cookie, drop the header
    for m in ["POST", "PUT", "DELETE", "PATCH"] {
        let (s, _, _) = c
            .raw(
                m,
                "/api/accounts",
                Some("application/json"),
                b"{}".to_vec(),
                &[],
            )
            .await;
        assert_eq!(
            s,
            StatusCode::BAD_REQUEST,
            "{m} without a CSRF token must be refused"
        );
    }
    c.csrf = "0".repeat(64);
    let (s, _, _) = c
        .raw(
            "PATCH",
            "/api/accounts",
            Some("application/json"),
            b"{}".to_vec(),
            &[],
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

// ─── Input that ends up in markup / spreadsheets ──────────────────────────────

#[tokio::test]
async fn icon_and_emoji_cannot_carry_markup() {
    let w = world().await;
    w.add_user("u1", "alice", "user").await;
    let mut c = w.login("alice").await;
    let (s, _) = c
        .json(
            "POST",
            "/api/accounts",
            Some(
                json!({"name": "x", "kind": "current", "icon": "bi-bank\"><img src=x onerror=1>",
                        "start_date": "2026-01-01"}),
            ),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    for emoji in [
        "<img src=x onerror=1>",
        "\"x",
        "",
        "this-is-way-too-long-for-an-emoji",
    ] {
        let (s, _) = c
            .json(
                "POST",
                "/api/categories",
                Some(json!({"name": "c", "emoji": emoji, "kind": "out"})),
            )
            .await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{emoji:?}");
    }
    let (s, _) = c
        .json(
            "POST",
            "/api/categories",
            Some(json!({"name": "ok", "emoji": "🛒", "kind": "out"})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
}

#[tokio::test]
async fn export_defuses_spreadsheet_formulas() {
    let w = world().await;
    w.add_user("u1", "alice", "user").await;
    let mut c = w.login("alice").await;
    let acct = c.simple_account("Main").await;
    let (s, _) = c
        .json(
            "POST",
            "/api/transactions",
            Some(
                json!({"account_id": acct, "date": "2026-02-03", "cents": -500,
                        "description": "=HYPERLINK(\"http://evil.test/\")"}),
            ),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let uri = format!("/api/export?from=2026-01-01&to=2026-12-31&account={acct}");
    let (s, body) = c.send("POST", &uri, None).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert!(!body.contains(";=HYPERLINK"), "formula left live: {body}");
    assert!(body.contains("'=HYPERLINK"), "{body}");
}

#[tokio::test]
async fn hsts_does_not_cover_the_whole_domain() {
    let mut cfg = (*config(1000)).clone();
    cfg.security.trust_proxy = true;
    let w = world_with(Arc::new(cfg)).await;
    let mut c = w.client();
    let (_, headers, _) = c.raw("GET", "/healthz", None, vec![], &[]).await;
    let hsts = headers
        .get("strict-transport-security")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        !hsts.contains("includeSubDomains") && !hsts.contains("preload"),
        "{hsts}"
    );
}

/// An administrator's pickers list the same things as anybody's; admin power
/// applies to ids they open, not to what they browse.
#[tokio::test]
async fn admin_category_list_is_not_flooded_with_private_items() {
    let w = world().await;
    w.add_user("adm", "admin", "admin").await;
    w.add_user("u1", "alice", "user").await;
    let mut alice = w.login("alice").await;
    let mut admin = w.login("admin").await;
    let (_, v) = alice
        .json(
            "POST",
            "/api/categories",
            Some(json!({"name": "Private", "emoji": "🔒", "kind": "out"})),
        )
        .await;
    let cat = v["id"].as_str().unwrap().to_string();
    let (_, list) = admin.json("GET", "/api/categories", None).await;
    assert!(list
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["id"] != cat.as_str()));
    // …yet by id the admin can still manage it.
    let (s, _) = admin
        .json(
            "PUT",
            &format!("/api/categories/{cat}"),
            Some(json!({"name": "Moderated"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
}
