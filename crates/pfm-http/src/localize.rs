//! Locale-aware error localization.
//!
//! Handlers and the [`crate::error::ApiError`] → `IntoResponse` mapping emit a
//! stable `code` (Italian fallback text). This tower middleware, applied as the
//! outermost layer, re-resolves the `detail` text for the caller's negotiated
//! `Accept-Language` locale using the [`pfm_i18n`] catalogue. Non-error
//! responses and non-problem bodies (e.g. static-file 404s) pass through
//! untouched.

use axum::body::Body;
use axum::http::header;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;
use http_body_util::BodyExt;
use pfm_i18n::{negotiate, translate_with, Locale, SUPPORTED};

/// Rewrite the `detail` field of JSON problem responses for the request locale.
pub async fn localize_errors(req: Request<Body>, next: Next) -> Response {
    let accept = req
        .headers()
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let res = next.run(req).await;

    // Only problem bodies (client / server errors) need localizing.
    let status = res.status();
    if !(status.is_client_error() || status.is_server_error()) {
        return res;
    }

    let (parts, body) = res.into_parts();
    let full = match body.collect().await {
        Ok(full) => full.to_bytes(),
        Err(_) => return Response::from_parts(parts, Body::empty()),
    };

    let mut value = match serde_json::from_slice::<serde_json::Value>(&full) {
        Ok(v) => v,
        // Not a JSON problem body (e.g. a static 404 page): leave unchanged.
        Err(_) => return Response::from_parts(parts, Body::from(full.to_vec())),
    };

    let code = match value.get("code").and_then(|c| c.as_str()) {
        Some(c) => c,
        // No stable code → nothing to localize.
        None => return Response::from_parts(parts, Body::from(full.to_vec())),
    };
    let args = value
        .get("args")
        .and_then(|a| a.as_str())
        .map(str::to_string);

    let locale = negotiate(accept.as_deref(), &SUPPORTED, Locale::It);
    let detail = translate_with(code, locale, args.as_deref());

    if let Some(obj) = value.as_object_mut() {
        obj.insert("detail".to_string(), serde_json::Value::String(detail));
    }

    let bytes = serde_json::to_vec(&value).unwrap_or_else(|_| full.to_vec());
    Response::from_parts(parts, Body::from(bytes))
}
