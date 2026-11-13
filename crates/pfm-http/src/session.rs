//! Session and CSRF token management.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use pfm_config::SecurityConfig;
use rand::Rng;
use sha2::{Digest, Sha512};
use tower_sessions::cookie::{Cookie, Key, SameSite};
use tower_sessions::{Expiry, MemoryStore, Session, SessionManagerLayer};

/// Minimum accepted length for a configured `session_secret`. Anything shorter
/// is a misconfiguration rather than a weak-but-usable key, and is rejected at
/// startup instead of being silently stretched.
const MIN_SESSION_SECRET_LEN: usize = 32;

/// Keys used within the session store.
pub const SESSION_USER_ID: &str = "user_id";
pub const SESSION_CSRF_TOKEN: &str = "csrf_token";
pub const SESSION_IMPERSONATED_USER_ID: &str = "impersonated_user_id";
/// Fingerprint of the password hash the session was opened with. A password
/// change (or admin reset) alters the hash and so ends every older session.
pub const SESSION_PW_FINGERPRINT: &str = "pw_fp";

/// Short, non-reversible fingerprint of a stored password hash.
pub fn password_fingerprint(password_hash: &str) -> String {
    let digest = Sha512::digest(password_hash.as_bytes());
    digest[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// Build a [`SessionManagerLayer`] backed by an in-memory store with signed cookies.
///
/// The signing key is derived from `security.session_secret` with SHA-512.
/// The previous implementation zero-padded the secret to 64 bytes, so a
/// 16-character secret produced a key with 48 bytes of constant zeros: the
/// key looked 512-bit but carried only the entropy of the original string in
/// its first bytes. Hashing spreads whatever entropy the secret has across the
/// whole key.
///
/// Secrets shorter than [`MIN_SESSION_SECRET_LEN`] are refused outright — a
/// too-short secret is a configuration error, and stretching it would only hide
/// the problem. When no secret is configured at all a random per-boot secret is
/// generated, a warning is logged and the secret is printed to stderr: the deployment is not insecure, but every
/// restart invalidates all sessions.
pub fn build_session_layer(
    security: &SecurityConfig,
) -> anyhow::Result<SessionManagerLayer<MemoryStore, tower_sessions::service::SignedCookie>> {
    let store = MemoryStore::default();

    // A configured secret is used as is; otherwise a random one is generated
    // and shown once on stderr, so it can be pasted into the configuration to
    // keep sessions across restarts. It goes to stderr only (never through
    // `tracing`, whose output is often shipped to log files): anyone holding it
    // can forge session cookies.
    let secret = match &security.session_secret {
        Some(secret) => {
            if secret.len() < MIN_SESSION_SECRET_LEN {
                anyhow::bail!(
                    "security.session_secret must be at least {MIN_SESSION_SECRET_LEN} characters \
                     (got {})",
                    secret.len()
                );
            }
            secret.clone()
        }
        None => {
            let secret = generate_csrf_token();
            tracing::warn!(
                "security.session_secret is not configured: using a random signing key. \
                 All sessions will be invalidated on every restart. Set session_secret in \
                 config.toml (or PFM_SECURITY__SESSION_SECRET) for a stable deployment."
            );
            eprintln!(
                "session_secret generated for this run (paste it into config.toml to keep \
                 sessions across restarts):\n  {secret}"
            );
            secret
        }
    };
    // `&digest[..]` rather than `digest.as_slice()`: the latter is a
    // deprecated generic-array 0.14 method, and `-D warnings` rejects it.
    // Indexing goes through `Deref<Target = [u8]>` and yields the same
    // 64-byte slice.
    let digest = Sha512::digest(secret.as_bytes());
    let key = Key::try_from(&digest[..])
        .map_err(|e| anyhow::anyhow!("deriving the session signing key: {e}"))?;

    let expiry = Expiry::OnInactivity(time::Duration::seconds(security.session_max_age_s as i64));

    Ok(SessionManagerLayer::new(store)
        .with_name(security.session_cookie_name.clone())
        .with_http_only(true)
        .with_same_site(SameSite::Lax)
        .with_secure(security.secure_cookies_effective())
        .with_expiry(expiry)
        .with_signed(key))
}

/// Generate a cryptographically random CSRF token (64 hex chars).
pub fn generate_csrf_token() -> String {
    let mut rng = rand::thread_rng();
    (0..32)
        .map(|_| format!("{:02x}", rng.gen_range(0u8..=255u8)))
        .collect()
}

/// Extractor that pulls the authenticated user ID from the session.
/// Fails with 401 if no valid session user is present.
pub struct AuthUserId(pub String);

impl<R> FromRequestParts<R> for AuthUserId
where
    R: Send + Sync,
{
    type Rejection = crate::error::ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &R) -> Result<Self, Self::Rejection> {
        let session = Session::from_request_parts(parts, _state)
            .await
            .map_err(|_| crate::error::ApiError::unauthorized())?;
        let user_id: String = session
            .get(SESSION_USER_ID)
            .await
            .map_err(|_| crate::error::ApiError::unauthorized())?
            .ok_or_else(crate::error::ApiError::unauthorized)?;
        Ok(Self(user_id))
    }
}

/// Extractor that provides access to the [`Session`] from request extensions.
pub struct SessionRef(pub Session);

impl<R> FromRequestParts<R> for SessionRef
where
    R: Send + Sync,
{
    type Rejection = crate::error::ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &R) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Session>()
            .cloned()
            .map(SessionRef)
            .ok_or_else(crate::error::ApiError::session_unavailable)
    }
}

/// Set the CSRF token in the session. Returns the token.
pub async fn establish_csrf(session: &Session) -> String {
    let token = generate_csrf_token();
    let _ = session.insert(SESSION_CSRF_TOKEN, token.clone()).await;
    token
}

/// Build a `Set-Cookie` header value that expires the CSRF token cookie.
/// Used at logout so the stale token is not left in the browser.
pub fn build_csrf_removal_cookie(security: &SecurityConfig) -> Cookie<'_> {
    Cookie::build((security.csrf_cookie_name.as_str(), ""))
        .path("/")
        .same_site(SameSite::Lax)
        .secure(security.secure_cookies_effective())
        .max_age(time::Duration::seconds(0))
        .build()
}

/// Build a `Set-Cookie` header value for the CSRF token cookie.
pub fn build_csrf_cookie<'a>(token: &'a str, security: &'a SecurityConfig) -> Cookie<'a> {
    Cookie::build((security.csrf_cookie_name.as_str(), token))
        .path("/")
        .same_site(SameSite::Lax)
        .secure(security.secure_cookies_effective())
        .max_age(time::Duration::seconds(security.session_max_age_s as i64))
        .build()
}
