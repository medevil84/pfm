//! Security headers middleware.
//!
//! A single axum middleware that injects the following headers into every
//! response:
//!
//! - `X-Content-Type-Options: nosniff`
//! - `Referrer-Policy: strict-origin-when-cross-origin`
//! - `Permissions-Policy: camera=(), microphone=(), geolocation=(), payment=()`
//! - `Content-Security-Policy` (from `SecurityConfig::csp`)
//! - `Cache-Control` differentiated: API → `no-cache, no-store`, static → `public, max-age=86400`
//! - `Strict-Transport-Security` (only when `trust_proxy` is true)
//!
//! Applied as a stateful middleware over an `Arc<SecurityConfig>` rather than
//! the whole `AppState`, so that the same layer can also wrap the static-asset
//! router and the setup guard page, neither of which has an `AppState`.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::header;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;
use pfm_config::SecurityConfig;

/// Whether a response must not be cached. Everything under `/api` is
/// per-user data; a prefix allow-list would silently miss every route added
/// later (that is how `/tags` and `/cards` ended up `public`). The optional
/// `/api/` mount prefix is also recognised in its stripped form so the check
/// behaves the same in the integration tests (which hit base paths).
/// Static files that change with every release under the same URL: HTML pages
/// (including the SPA routes that fall back to `index.html`), scripts, styles,
/// JSON catalogues and the manifest. Vendored bundles, fonts and icons are the
/// only ones safe to cache for a day.
fn is_revalidated_asset(path: &str) -> bool {
    if path.starts_with("/vendor/") || path.starts_with("/icons/") {
        return false;
    }
    let last = path.rsplit('/').next().unwrap_or("");
    // "/" and extension-less paths are SPA routes served as index.html.
    if last.is_empty() || !last.contains('.') {
        return true;
    }
    [".html", ".js", ".css", ".json", ".webmanifest"]
        .iter()
        .any(|ext| last.ends_with(ext))
}

fn is_api_path(path: &str) -> bool {
    if path == "/api" || path.starts_with("/api/") {
        return true;
    }
    const BARE_PREFIXES: &[&str] = &[
        "/auth",
        "/accounts",
        "/transactions",
        "/transfers",
        "/categories",
        "/recurrences",
        "/stats",
        "/import",
        "/export",
        "/users",
        "/admin",
        "/setup",
        "/tags",
        "/cards",
        "/holiday-calendars",
        "/deposits",
        "/rules",
    ];
    path == "/healthz"
        || path == "/readyz"
        || BARE_PREFIXES
            .iter()
            .any(|p| path == *p || path.starts_with(&format!("{p}/")))
}

/// Tower/axum middleware that injects all security headers into the
/// response. Must be applied with `middleware::from_fn_with_state`.
pub async fn security_headers(
    State(security): State<Arc<SecurityConfig>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();
    let mut res = next.run(req).await;

    let headers = res.headers_mut();

    // ── X-Content-Type-Options ──────────────────────────────────────────
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());

    // ── Referrer-Policy ─────────────────────────────────────────────────
    headers.insert(
        header::REFERRER_POLICY,
        "strict-origin-when-cross-origin".parse().unwrap(),
    );

    // ── Permissions-Policy ──────────────────────────────────────────────
    headers.insert(
        "Permissions-Policy",
        "camera=(), microphone=(), geolocation=(), payment=()"
            .parse()
            .unwrap(),
    );

    // ── Content-Security-Policy ─────────────────────────────────────────
    // The configured policy plus the anti-framing / anti-hijack directives an
    // older config file may lack (frame-ancestors, form-action, base-uri,
    // object-src).
    if let Ok(csp) = security.effective_csp().parse() {
        headers.insert(header::CONTENT_SECURITY_POLICY, csp);
    }
    // Legacy equivalent of `frame-ancestors 'none'`.
    headers.insert(header::X_FRAME_OPTIONS, "DENY".parse().unwrap());

    // ── Cache-Control (differentiated) ──────────────────────────────────
    if is_api_path(&path) {
        headers.insert(
            header::CACHE_CONTROL,
            "no-cache, no-store, must-revalidate".parse().unwrap(),
        );
        headers.insert(header::PRAGMA, "no-cache".parse().unwrap());
    } else if is_revalidated_asset(&path) {
        // Our own pages, scripts and catalogues have no hash in the URL: with
        // `max-age=86400` a browser keeps serving the previous index.html /
        // app.js for a day after a redeploy (the new CSS and settings
        // controls "do not appear"). `no-cache` still lets it store the file
        // but forces a revalidation on every use.
        headers.insert(header::CACHE_CONTROL, "no-cache".parse().unwrap());
    } else {
        headers.insert(
            header::CACHE_CONTROL,
            "public, max-age=86400".parse().unwrap(),
        );
    }

    // ── Strict-Transport-Security ───────────────────────────────────────
    // Only when the app sits behind a TLS-terminating proxy. In standalone
    // mode without TLS, HSTS would break the connection.
    if security.secure_cookies_effective() {
        headers.insert(
            "Strict-Transport-Security",
            "max-age=31536000".parse().unwrap(),
        );
    }

    res
}

#[cfg(test)]
mod tests {
    use super::is_revalidated_asset;

    #[test]
    fn app_files_are_revalidated_vendor_and_icons_are_not() {
        for p in [
            "/",
            "/index.html",
            "/app.js",
            "/sw.js",
            "/setup_wizard.html",
            "/i18n/it.json",
            "/manifest.webmanifest",
            "/statistiche", // SPA route served as index.html
        ] {
            assert!(is_revalidated_asset(p), "{p}");
        }
        for p in [
            "/vendor/bootstrap.min.js",
            "/vendor/fonts/x.woff2",
            "/icons/icon-192.png",
        ] {
            assert!(!is_revalidated_asset(p), "{p}");
        }
    }
}
