//! RFC 7807-style error responses.
//!
//! Every application error is expressed as a **stable code** (e.g.
//! `errors.account_not_found`) rather than a hard-coded human string. The
//! human-readable text for the caller's locale is resolved at response time from
//! the [`pfm_i18n`] catalogue by the `localize_errors` middleware, which reads
//! the `Accept-Language` header. The [`ApiError::into_response`] implementation
//! produces an Italian fallback so the body is meaningful even without the
//! middleware (e.g. in tests that call `into_response` directly).

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use pfm_i18n::{translate_with, Locale};
use serde::Serialize;

/// Structured error body conforming to RFC 9457 (Problem Details).
#[derive(Debug, Serialize)]
pub struct ProblemBody {
    pub r#type: String,
    pub title: String,
    pub status: u16,
    pub detail: String,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
}

/// Maps an application error to an HTTP response with a JSON problem body.
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    args: Option<String>,
}

impl ApiError {
    /// Build an error from a stable `code` with no dynamic suffix.
    pub fn new(status: StatusCode, code: &'static str) -> Self {
        Self {
            status,
            code,
            args: None,
        }
    }

    /// Build an error from a stable `code` with a dynamic `args` suffix (a
    /// service-generated message or list of identifiers), appended verbatim.
    pub fn with_args(status: StatusCode, code: &'static str, args: impl Into<String>) -> Self {
        let args = args.into();
        Self {
            status,
            code,
            args: if args.trim().is_empty() {
                None
            } else {
                Some(args)
            },
        }
    }

    // ── convenience constructors keyed by HTTP status ──────────────────────────

    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "errors.not_authenticated")
    }

    pub fn forbidden() -> Self {
        Self::new(StatusCode::FORBIDDEN, "errors.access_denied")
    }

    pub fn not_found(code: &'static str) -> Self {
        Self::new(StatusCode::NOT_FOUND, code)
    }

    pub fn bad_request(code: &'static str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code)
    }

    pub fn too_many_requests() -> Self {
        Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            "errors.too_many_login_attempts",
        )
    }

    pub fn conflict(code: &'static str) -> Self {
        Self::new(StatusCode::CONFLICT, code)
    }

    pub fn unprocessable(code: &'static str, args: Option<String>) -> Self {
        Self::with_args(
            StatusCode::UNPROCESSABLE_ENTITY,
            code,
            args.unwrap_or_default(),
        )
    }

    /// A server-side failure. The detail (database / driver error text, file
    /// paths, …) goes to the log only; the client gets the stable
    /// `errors.internal` code and nothing it could use to probe the schema.
    pub fn internal(args: Option<String>) -> Self {
        if let Some(detail) = args.as_deref() {
            tracing::error!(detail, "internal error");
        }
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "errors.internal")
    }

    pub fn session_unavailable() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "errors.session_unavailable",
        )
    }
}

impl From<pfm_service::AuthError> for ApiError {
    fn from(e: pfm_service::AuthError) -> Self {
        use pfm_service::AuthError::*;
        match e {
            InvalidCredentials => Self::new(StatusCode::UNAUTHORIZED, "errors.invalid_credentials"),
            AccountInactive => Self::forbidden_from("errors.account_inactive"),
            Busy => Self::too_many_requests(),
            UserNotFound => Self::not_found("errors.user_not_found"),
            IncorrectPassword => Self::bad_request("errors.incorrect_password"),
            Internal(msg) => Self::internal(Some(msg)),
        }
    }
}

impl ApiError {
    fn forbidden_from(code: &'static str) -> Self {
        Self::new(StatusCode::FORBIDDEN, code)
    }
}

impl From<pfm_service::UserAdminError> for ApiError {
    fn from(e: pfm_service::UserAdminError) -> Self {
        use pfm_service::UserAdminError::*;
        match e {
            UserNotFound => Self::not_found("errors.user_not_found"),
            AccountNotFound => Self::not_found("errors.account_not_found"),
            UsernameTaken => Self::conflict("errors.username_taken"),
            EmailTaken => Self::conflict("errors.email_taken"),
            AccountsNeedReassignment(names) => Self::with_args(
                StatusCode::UNPROCESSABLE_ENTITY,
                "errors.reassignment_required",
                names,
            ),
            AccountHasTransactions => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "errors.account_has_transactions",
            ),
            Validation(msg) => {
                Self::with_args(StatusCode::UNPROCESSABLE_ENTITY, "errors.validation", msg)
            }
            LastAdmin => Self::conflict("errors.last_admin"),
            Internal(msg) => Self::internal(Some(msg)),
        }
    }
}

impl From<pfm_service::ImpersonationError> for ApiError {
    fn from(e: pfm_service::ImpersonationError) -> Self {
        use pfm_service::ImpersonationError::*;
        match e {
            UserNotFound => Self::not_found("errors.user_not_found"),
            TargetInactive => Self::forbidden_from("errors.target_user_inactive"),
            NotImpersonating => Self::bad_request("errors.not_impersonating"),
            Internal(msg) => Self::internal(Some(msg)),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // Italian fallback so the detail is meaningful without the request's
        // locale; the localize_errors middleware re-resolves it for the caller.
        let detail = translate_with(self.code, Locale::It, self.args.as_deref());
        let body = ProblemBody {
            r#type: "about:blank".to_string(),
            title: self
                .status
                .canonical_reason()
                .unwrap_or("Error")
                .to_string(),
            status: self.status.as_u16(),
            detail,
            code: self.code.to_string(),
            args: self.args.clone(),
        };
        (
            self.status,
            axum::Json(serde_json::to_value(&body).unwrap()),
        )
            .into_response()
    }
}
