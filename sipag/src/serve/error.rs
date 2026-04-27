//! HTTP error type for sipag's API surface.
//!
//! Translates `AuthError` plus a handful of HTTP-shape variants into
//! status codes + opaque JSON bodies. Server-side error context lives in
//! the `tracing` log; the client gets a stable `code` string and a
//! generic detail. Never render the full error chain to the wire.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use serde_json::json;
use sipag_core::auth::AuthError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("bad request: {0}")]
    BadRequest(&'static str),

    #[error("unauthorized")]
    Unauthorized,

    #[error("forbidden: {0}")]
    Forbidden(&'static str),

    #[error("conflict: {0}")]
    Conflict(&'static str),

    #[error("auth: {0}")]
    Auth(#[from] AuthError),

    #[error("internal: {0}")]
    Internal(#[from] anyhow::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::BadRequest(detail) => json_status(StatusCode::BAD_REQUEST, "bad_request", detail),
            ApiError::Unauthorized => {
                json_status(StatusCode::UNAUTHORIZED, "unauthorized", "sign in")
            }
            ApiError::Forbidden(detail) => json_status(StatusCode::FORBIDDEN, "forbidden", detail),
            ApiError::Conflict(detail) => json_status(StatusCode::CONFLICT, "conflict", detail),
            ApiError::Internal(err) => {
                tracing::warn!(error = %err, "internal error");
                json_status(StatusCode::INTERNAL_SERVER_ERROR, "internal", "try again")
            }
            ApiError::Auth(err) => map_auth_error(err),
        }
    }
}

fn map_auth_error(err: AuthError) -> Response {
    use AuthError::*;
    match err {
        Io { .. } | Parse(_) | Hash(_) | UnsupportedVersion(_) | WebAuthnConfig(_) => {
            tracing::warn!(error = %err, "auth: server-side failure");
            json_status(StatusCode::INTERNAL_SERVER_ERROR, "internal", "try again")
        }
        WebAuthn(_) | ChallengeNotFound => {
            tracing::warn!(error = %err, "auth: ceremony failure");
            json_status(
                StatusCode::UNAUTHORIZED,
                "auth.ceremony_failed",
                "retry sign-in",
            )
        }
        TooManyPendingChallenges => {
            tracing::warn!("auth: too many pending challenges");
            json_status(
                StatusCode::SERVICE_UNAVAILABLE,
                "auth.busy",
                "retry shortly",
            )
        }
        StateConflict(reason) => {
            tracing::warn!(reason, "auth: state conflict");
            json_status(StatusCode::CONFLICT, "conflict", reason)
        }
        LastCredentialRemoval => json_status(
            StatusCode::CONFLICT,
            "auth.last_credential",
            "cannot remove the only credential",
        ),
    }
}

fn json_status(status: StatusCode, code: &str, detail: &str) -> Response {
    (
        status,
        Json(json!({ "error": code, "detail": detail })),
    )
        .into_response()
}
