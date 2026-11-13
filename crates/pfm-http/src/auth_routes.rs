//! Auth route handlers.
//!
//! CSRF validation for mutating endpoints is handled by the [`crate::csrf::csrf_middleware`]
//! layer; these handlers assume the request has already passed CSRF checks.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{header, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use std::net::SocketAddr;

use crate::dto::{
    ChangePasswordRequest, LoginRequest, LoginResponse, MeResponse, PrefsDto, SetLocaleRequest,
    UserDto,
};
use crate::error::ApiError;
use crate::session::{
    build_csrf_cookie, build_csrf_removal_cookie, establish_csrf, password_fingerprint, AuthUserId,
    SessionRef, SESSION_PW_FINGERPRINT, SESSION_USER_ID,
};
use crate::state::AppState;

use crate::validate::{
    check_len, check_password, require_json_content_type, MAX_PASSWORD_LEN, MAX_USERNAME_LEN,
};
use pfm_config::resolve_client_ip;
use pfm_domain::{Credentials, UserPrefs};

/// Build the auth routes router.
///
/// The caller is responsible for wrapping this router with:
/// - `SessionManagerLayer` (outermost)
/// - `csrf_middleware` (inside the session layer)
pub fn auth_routes(_state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/me", get(me))
        .route("/auth/password", put(change_password))
        .route("/auth/locale", put(set_locale))
        .route("/auth/prefs", get(get_prefs).put(update_prefs))
}

/// Address used for throttling: IPv6 clients are grouped by /64, the smallest
/// block a subscriber normally receives, so one household cannot dodge the
/// limit (nor fill the limiter) by rotating through its address space.
fn throttle_key(ip: &str) -> String {
    match ip.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V6(v6)) => {
            let seg = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", seg[0], seg[1], seg[2], seg[3])
        }
        _ => ip.to_string(),
    }
}

/// `PUT /auth/locale`
pub async fn set_locale(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    req: Request<Body>,
) -> Result<StatusCode, ApiError> {
    require_json_content_type(req.headers())?;
    let body_bytes = axum::body::to_bytes(req.into_body(), 1_048_576)
        .await
        .map_err(|_| ApiError::bad_request("errors.invalid_json"))?;
    let body: SetLocaleRequest = serde_json::from_slice(&body_bytes)
        .map_err(|_| ApiError::bad_request("errors.invalid_json"))?;

    let locale = pfm_i18n::SUPPORTED
        .iter()
        .find(|l| l.tag() == body.locale)
        .ok_or_else(|| ApiError::bad_request("errors.invalid_locale"))?;
    state.auth.set_locale(&user_id, locale.tag()).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /auth/login`
///
/// Uses `Request<Body>` directly so we can read the `X-Forwarded-For` header
/// alongside the JSON body (axum does not allow `Request` alongside `Json`).
pub async fn login(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    SessionRef(session): SessionRef,
    req: Request<Body>,
) -> Result<Response, ApiError> {
    let security = &state.security;

    let xff = req
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok());
    let ip = resolve_client_ip(addr.ip(), xff, security);

    // Behind a reverse proxy with `trust_proxy = false` every client looks like
    // the proxy, so they all share one throttling budget. Say so once.
    if xff.is_some() && !security.trust_proxy {
        static WARNED: std::sync::Once = std::sync::Once::new();
        WARNED.call_once(|| {
            tracing::warn!(
                "login request carries X-Forwarded-For but security.trust_proxy is off: all \
                 clients are throttled as one address ({ip}). Behind a proxy set trust_proxy = \
                 true and trusted_proxies = [\"<proxy ip>\"]"
            );
        });
    }
    let throttle_ip = throttle_key(&ip);

    // Login is CSRF-exempt (there is no session to bind a token to), so the
    // body must at least be something a cross-site form cannot produce.
    require_json_content_type(req.headers())?;

    // Per-address budget first: it counts every attempt from one client no
    // matter which username it tries.
    if state.rate_limiter.check_ip(&throttle_ip) {
        return Err(ApiError::too_many_requests());
    }

    let body_bytes = axum::body::to_bytes(req.into_body(), 1_048_576)
        .await
        .map_err(|_| ApiError::bad_request("errors.invalid_json"))?;
    let body: LoginRequest = serde_json::from_slice(&body_bytes)
        .map_err(|_| ApiError::bad_request("errors.invalid_json"))?;

    // Input validation
    check_len(&body.username, MAX_USERNAME_LEN, "username")?;
    if body.password.len() > MAX_PASSWORD_LEN {
        return Err(ApiError::with_args(
            StatusCode::BAD_REQUEST,
            "errors.validation",
            "password too long",
        ));
    }

    let rate_key = format!("{throttle_ip}:{}", body.username);
    if state.rate_limiter.check(&rate_key) {
        return Err(ApiError::too_many_requests());
    }

    let creds = Credentials::new(&body.username, &body.password);
    let user = state.auth.login(&creds, Some(ip), None).await?;

    if !user.active {
        return Err(ApiError::new(
            axum::http::StatusCode::FORBIDDEN,
            "errors.account_inactive",
        ));
    }

    let user_id = user.id.clone();
    let auth_user = user.auth();

    // Session fixation: issue a fresh session id now that the caller has
    // authenticated, so an id planted before login cannot be reused afterwards.
    // Session data is preserved across the cycle.
    let _ = session.cycle_id().await;

    let _ = session.insert(SESSION_USER_ID, user_id).await;
    let _ = session
        .insert(
            SESSION_PW_FINGERPRINT,
            password_fingerprint(&user.password_hash),
        )
        .await;
    let csrf_token = establish_csrf(&session).await;
    let csrf_cookie = build_csrf_cookie(&csrf_token, security);
    let csrf_cookie_hdr = csrf_cookie.to_string().parse().unwrap();

    let mut response = (
        StatusCode::OK,
        Json(LoginResponse {
            user: UserDto::from(&auth_user),
            csrf_token,
        }),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, csrf_cookie_hdr);

    Ok(response)
}

/// `POST /auth/logout`
pub async fn logout(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    SessionRef(session): SessionRef,
    req: Request<Body>,
) -> Result<Response, ApiError> {
    let user_id: Option<String> = session.get(SESSION_USER_ID).await.ok().flatten();

    if let Some(uid) = &user_id {
        let xff = req
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok());
        let ip = resolve_client_ip(addr.ip(), xff, &state.security);
        let _ = state.auth.logout(uid, Some(ip), None).await;
    }

    // Drop the whole session rather than just its keys: `flush` deletes the
    // record from the store and clears the cookie, so the id cannot be reused.
    let _ = session.flush().await;

    let mut response = StatusCode::NO_CONTENT.into_response();
    let removal = build_csrf_removal_cookie(&state.security);
    if let Ok(value) = removal.to_string().parse() {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    Ok(response)
}

/// `GET /auth/me`
pub async fn me(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
) -> Result<Json<MeResponse>, ApiError> {
    let user = state
        .auth
        .get_user_by_id(&user_id)
        .await?
        .ok_or_else(|| ApiError::not_found("errors.user_not_found"))?;

    Ok(Json(MeResponse {
        user: UserDto::from(&user.auth()),
    }))
}

/// `PUT /auth/password`
pub async fn change_password(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    AuthUserId(user_id): AuthUserId,
    SessionRef(session): SessionRef,
    req: Request<Body>,
) -> Result<StatusCode, ApiError> {
    require_json_content_type(req.headers())?;
    let xff = req
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok());
    let ip = resolve_client_ip(addr.ip(), xff, &state.security);

    let body_bytes = axum::body::to_bytes(req.into_body(), 1_048_576)
        .await
        .map_err(|_| ApiError::bad_request("errors.invalid_json"))?;
    let body: ChangePasswordRequest = serde_json::from_slice(&body_bytes)
        .map_err(|_| ApiError::bad_request("errors.invalid_json"))?;

    check_password(&body.new_password)?;
    let cmd = pfm_domain::ChangePassword::new(&body.current_password, &body.new_password);
    state
        .auth
        .change_password(&user_id, &cmd, Some(ip), None)
        .await?;
    // The password hash changed, which ends every other session of this user
    // (see `session_guard`); keep the one that made the change.
    crate::session_guard::refresh_session_fingerprint(&state, &session, &user_id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /auth/prefs`
pub async fn get_prefs(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
) -> Result<Json<PrefsDto>, ApiError> {
    let prefs = state.prefs.get(&user_id).await?;
    Ok(Json(PrefsDto::from(&prefs)))
}

/// `PUT /auth/prefs`
pub async fn update_prefs(
    State(state): State<Arc<AppState>>,
    AuthUserId(user_id): AuthUserId,
    Json(body): Json<PrefsDto>,
) -> Result<StatusCode, ApiError> {
    if !state.holidays.exists(&body.holiday_calendar).await? {
        return Err(ApiError::unprocessable(
            "errors.validation",
            Some(format!(
                "invalid holiday_calendar: {}",
                body.holiday_calendar
            )),
        ));
    }
    for (field, value) in [
        ("color_pos", &body.color_pos),
        ("color_neg", &body.color_neg),
    ] {
        if !pfm_domain::is_valid_amount_color(value) {
            return Err(ApiError::unprocessable(
                "errors.validation",
                Some(format!("invalid {field}: expected #rrggbb or empty")),
            ));
        }
    }
    let prefs = UserPrefs::from(body);
    state.prefs.update(&user_id, &prefs).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::throttle_key;

    #[test]
    fn ipv6_clients_are_grouped_by_slash_64() {
        assert_eq!(
            throttle_key("2001:db8:1:2:aaaa:bbbb:cccc:dddd"),
            throttle_key("2001:db8:1:2::1")
        );
        assert_ne!(
            throttle_key("2001:db8:1:2::1"),
            throttle_key("2001:db8:1:3::1")
        );
        assert_eq!(throttle_key("203.0.113.9"), "203.0.113.9");
    }
}
