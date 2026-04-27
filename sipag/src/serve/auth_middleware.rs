//! Request-level authentication.
//!
//! Public paths (`/login`, `/api/auth/*`, static assets) are admitted
//! unconditionally so the unauthenticated user can reach the WebAuthn
//! flows. For everything else, the request must either:
//!   (a) come from loopback peer + loopback Host header, or
//!   (b) carry a valid unexpired session cookie.
//!
//! The Authenticated extractor returns a rich `AuthContext` so handlers
//! that need the credential or the plaintext token (logout) don't have
//! to re-parse the cookie.

use crate::serve::access::AccessMethod;
use crate::serve::cookie::extract_session_token;
use crate::serve::state::AppState;
use axum::{
    extract::{ConnectInfo, FromRequestParts, State},
    http::{header, request::Parts, StatusCode, Uri},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use sipag_core::auth::{Credential, Session};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::SystemTime;

/// Fallback peer when the request didn't go through
/// `into_make_service_with_connect_info` (in-process tests, mostly).
/// Non-loopback so `AccessMethod::classify` falls to `Remote` —
/// fail-closed: a missing ConnectInfo must NOT silently grant
/// localhost-bypass.
const FALLBACK_PEER: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 0);

#[allow(clippy::large_enum_variant)]
#[allow(dead_code)] // session field + credential_id helper are part of the public surface for handlers we haven't wired yet.
#[derive(Debug, Clone)]
pub enum AuthContext {
    Localhost,
    Remote {
        session: Session,
        credential: Credential,
        /// Plaintext session-token as received from the client's `Cookie`
        /// header. Preserved here so handlers can call `remove_session`
        /// without going back to the raw header. Never logged.
        plaintext_token: String,
    },
}

#[allow(dead_code)]
impl AuthContext {
    pub fn credential_id(&self) -> Option<&str> {
        match self {
            Self::Localhost => None,
            Self::Remote { credential, .. } => Some(&credential.id),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Authenticated(pub AuthContext);

#[derive(Debug)]
pub struct AuthRejection;

impl IntoResponse for AuthRejection {
    fn into_response(self) -> Response {
        (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
    }
}

#[axum::async_trait]
impl FromRequestParts<AppState> for Authenticated {
    type Rejection = AuthRejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let peer = ConnectInfo::<SocketAddr>::from_request_parts(parts, state)
            .await
            .map(|ConnectInfo(p)| p)
            .unwrap_or(FALLBACK_PEER);
        let host = parts
            .headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok());
        let access = AccessMethod::classify(peer, host);
        if matches!(access, AccessMethod::Localhost) {
            return Ok(Authenticated(AuthContext::Localhost));
        }

        let token = parts
            .headers
            .get(header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .and_then(extract_session_token)
            .ok_or(AuthRejection)?;

        let snapshot = state.auth_store.snapshot().await;
        let now = SystemTime::now();
        let session = snapshot
            .valid_session(&token, now)
            .ok_or(AuthRejection)?
            .clone();
        let credential = snapshot
            .find_credential(&session.credential_id)
            .ok_or(AuthRejection)?
            .clone();

        Ok(Authenticated(AuthContext::Remote {
            session,
            credential,
            plaintext_token: token,
        }))
    }
}

/// Public path matcher. Anything matched here bypasses the gate.
fn is_public_path(uri: &Uri) -> bool {
    let path = uri.path();
    if path.starts_with("/api/auth/") {
        return true;
    }
    matches!(path, "/login" | "/style.css" | "/favicon.ico") || path.starts_with("/js/")
}

/// Ensure every request has a ConnectInfo extension so handlers that
/// rely on it via `ConnectInfo(peer)` never 500. In production the
/// `into_make_service_with_connect_info` layer fills this in with the
/// real peer; this middleware is the safety net for missing-extension
/// edge cases. Falls back to a non-loopback address so the gate
/// classifies as remote — fail-closed.
pub async fn ensure_connect_info(
    mut request: axum::extract::Request,
    next: Next,
) -> Response {
    if request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .is_none()
    {
        request.extensions_mut().insert(ConnectInfo(FALLBACK_PEER));
    }
    next.run(request).await
}

/// Test-only variant: fall back to loopback peer when ConnectInfo is
/// missing. Lets axum-test (which doesn't set up
/// `into_make_service_with_connect_info`) exercise the
/// localhost-bypass path with a loopback Host header.
pub async fn ensure_loopback_connect_info(
    mut request: axum::extract::Request,
    next: Next,
) -> Response {
    const TEST_PEER: SocketAddr =
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0);
    if request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .is_none()
    {
        request.extensions_mut().insert(ConnectInfo(TEST_PEER));
    }
    next.run(request).await
}

/// Page-level gate. Wraps the router so unauthenticated requests to
/// non-public paths get either a 302 (browser) or 401 JSON (API).
pub async fn gate(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    if is_public_path(request.uri()) {
        return next.run(request).await;
    }

    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0)
        .unwrap_or(FALLBACK_PEER);
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok());
    if matches!(AccessMethod::classify(peer, host), AccessMethod::Localhost) {
        return next.run(request).await;
    }

    // Remote — require a valid session.
    let token = request
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(extract_session_token);

    if let Some(plaintext) = token {
        let now = SystemTime::now();
        let snapshot = state.auth_store.snapshot().await;
        if snapshot.valid_session(&plaintext, now).is_some() {
            // Slide the session forward best-effort.
            let _ = state
                .auth_store
                .transact(|s| {
                    let next = s
                        .touch_session(&plaintext, now)
                        .renew_session(&plaintext, now + sipag_core::auth::SESSION_TTL);
                    Ok((next, ()))
                })
                .await;
            return next.run(request).await;
        }
    }

    let path = request.uri().path();
    if path.starts_with("/api/") {
        (
            StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({
                "error": "unauthorized",
                "detail": "sign in at /login"
            })),
        )
            .into_response()
    } else {
        Redirect::to("/login").into_response()
    }
}
