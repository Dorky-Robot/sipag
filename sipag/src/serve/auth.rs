//! Authentication HTTP routes.
//!
//! Eight endpoints:
//!
//! - `GET  /api/auth/status`          — public; access mode + install state
//! - `POST /api/auth/register/start`  — localhost-only, fresh install
//! - `POST /api/auth/register/finish` — localhost-only, fresh install; mints session
//! - `POST /api/auth/login/start`     — public
//! - `POST /api/auth/login/finish`    — public; updates counter + mints session
//! - `POST /api/auth/pair/start`      — public, setup-token-gated
//! - `POST /api/auth/pair/finish`     — public; links credential to token + mints session
//! - `POST /api/auth/logout`          — auth-only; localhost → 409
//!
//! Setup-token management lives in `tokens.rs`.

use crate::serve::auth_middleware::{AuthContext, Authenticated};
use crate::serve::cookie::{build_clear_cookie, build_set_cookie};
use crate::serve::error::ApiError;
use crate::serve::state::AppState;
use axum::{
    extract::{ConnectInfo, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sipag_core::auth::webauthn_wire::{
    CreationChallengeResponse, PublicKeyCredential, RegisterPublicKeyCredential,
    RequestChallengeResponse,
};
use sipag_core::auth::{AuthError, ChallengeId, Session, SESSION_TTL};
use std::net::SocketAddr;
use std::time::SystemTime;

use crate::serve::access::AccessMethod;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/auth/status", get(status))
        .route("/api/auth/register/start", post(register_start))
        .route("/api/auth/register/finish", post(register_finish))
        .route("/api/auth/login/start", post(login_start))
        .route("/api/auth/login/finish", post(login_finish))
        .route("/api/auth/pair/start", post(pair_start))
        .route("/api/auth/pair/finish", post(pair_finish))
        .route("/api/auth/logout", post(logout))
}

// ---------- /api/auth/status ----------

#[derive(Debug, Serialize)]
struct AuthStatus {
    /// `"localhost"` or `"remote"` — the binary access model.
    access_method: &'static str,
    /// True when at least one credential is registered. A fresh install
    /// reports `false` and the client routes to register.
    has_credentials: bool,
    /// True when the current request would pass the `Authenticated`
    /// extractor.
    authenticated: bool,
}

async fn status(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    authed: Option<Authenticated>,
) -> Json<AuthStatus> {
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    let access = AccessMethod::classify(peer, host);
    let snap = state.auth_store.snapshot().await;
    Json(AuthStatus {
        access_method: match access {
            AccessMethod::Localhost => "localhost",
            AccessMethod::Remote => "remote",
        },
        has_credentials: !snap.credentials.is_empty(),
        authenticated: authed.is_some(),
    })
}

// ---------- shared shapes ----------

#[derive(Debug, Serialize)]
struct ChallengeStartResponse<T: Serialize> {
    challenge_id: ChallengeId,
    options: T,
}

#[derive(Debug, Deserialize)]
struct RegisterFinishRequest {
    challenge_id: ChallengeId,
    response: RegisterPublicKeyCredential,
}

#[derive(Debug, Deserialize)]
struct LoginFinishRequest {
    challenge_id: ChallengeId,
    response: PublicKeyCredential,
}

/// Shared response shape for register/login/pair on success.
#[derive(Debug, Serialize)]
struct AuthFinishResponse {
    credential_id: String,
    csrf_token: String,
}

// ---------- first-device register (localhost-only) ----------

async fn register_start(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<ChallengeStartResponse<CreationChallengeResponse>>, ApiError> {
    require_localhost(peer, &headers)?;

    // Get-or-init the stable user handle AND enforce the "no credentials
    // yet" invariant atomically. The same re-check appears in
    // `register_finish`'s transact closure.
    let (user_handle, credentials) = state
        .auth_store
        .transact(|s| {
            if !s.credentials.is_empty() {
                return Err(AuthError::StateConflict(
                    "instance already initialised; additional devices must pair via setup token",
                ));
            }
            let (uh, next) = s.user_handle_or_init();
            let creds = next.credentials.clone();
            Ok((next, (uh, creds)))
        })
        .await?;

    let (id, ccr) = state.webauthn.start_registration(
        user_handle,
        "sipag",
        "Sipag",
        &credentials,
        SystemTime::now(),
    )?;
    Ok(Json(ChallengeStartResponse {
        challenge_id: id,
        options: ccr,
    }))
}

async fn register_finish(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<RegisterFinishRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_localhost(peer, &headers)?;

    let now = SystemTime::now();
    let credential = state
        .webauthn
        .finish_registration(&body.challenge_id, &body.response, now)
        .inspect_err(|e| {
            tracing::warn!(
                challenge_id = %body.challenge_id,
                error = %e,
                "registration ceremony failed"
            );
        })?;

    let new_credential = credential.clone();
    let cookie_secure = state.cookie_secure;
    let (plaintext_token, minted_session) = state
        .auth_store
        .transact(move |s| {
            if !s.credentials.is_empty() {
                return Err(AuthError::StateConflict(
                    "instance already initialised by a concurrent registration",
                ));
            }
            let (plaintext, session) = Session::mint(new_credential.id.clone(), now, SESSION_TTL);
            let next = s
                .upsert_credential(new_credential.clone())
                .upsert_session(session.clone());
            Ok((next, (plaintext, session)))
        })
        .await?;

    tracing::info!(
        credential_id = %credential.id,
        "registered first device; session minted"
    );

    Ok(session_cookie_response(
        StatusCode::CREATED,
        &plaintext_token,
        cookie_secure,
        Json(AuthFinishResponse {
            credential_id: credential.id,
            csrf_token: minted_session.csrf_token,
        }),
    ))
}

// ---------- login ----------

async fn login_start(
    State(state): State<AppState>,
) -> Result<Json<ChallengeStartResponse<RequestChallengeResponse>>, ApiError> {
    let snap = state.auth_store.snapshot().await;
    if snap.credentials.is_empty() {
        return Err(ApiError::Conflict(
            "no credentials registered; register first device on localhost",
        ));
    }
    let (id, rcr) = state
        .webauthn
        .start_authentication(&snap.credentials, SystemTime::now())?;
    Ok(Json(ChallengeStartResponse {
        challenge_id: id,
        options: rcr,
    }))
}

async fn login_finish(
    State(state): State<AppState>,
    Json(body): Json<LoginFinishRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let now = SystemTime::now();
    let verified = state
        .webauthn
        .finish_authentication(&body.challenge_id, &body.response, now)
        .inspect_err(|e| {
            tracing::warn!(
                challenge_id = %body.challenge_id,
                error = %e,
                "login ceremony failed"
            );
        })?;

    let credential_id = verified.credential_id.clone();
    let cookie_secure = state.cookie_secure;
    let (plaintext_token, minted_session) = state
        .auth_store
        .transact(move |s| {
            let cred = s.find_credential(&credential_id).ok_or(
                AuthError::StateConflict("credential revoked during authentication ceremony"),
            )?;
            let updated = cred.apply_authentication(&verified.result)?;
            let (plaintext, session) = Session::mint(credential_id.clone(), now, SESSION_TTL);
            let mut next = s.clone();
            if let Some(updated_cred) = updated {
                next = next.upsert_credential(updated_cred);
            }
            next = next.upsert_session(session.clone());
            Ok((next, (plaintext, session)))
        })
        .await?;

    tracing::info!(
        credential_id = %verified.credential_id,
        "login succeeded; session minted"
    );

    Ok(session_cookie_response(
        StatusCode::OK,
        &plaintext_token,
        cookie_secure,
        Json(AuthFinishResponse {
            credential_id: verified.credential_id,
            csrf_token: minted_session.csrf_token,
        }),
    ))
}

// ---------- pair (setup-token-gated registration) ----------

#[derive(Debug, Deserialize)]
struct PairStartRequest {
    setup_token: String,
}

#[derive(Debug, Serialize)]
struct PairStartResponse {
    challenge_id: ChallengeId,
    setup_token_id: String,
    options: CreationChallengeResponse,
}

#[derive(Debug, Deserialize)]
struct PairFinishRequest {
    challenge_id: ChallengeId,
    setup_token_id: String,
    response: RegisterPublicKeyCredential,
}

const SETUP_TOKEN_MAX_LEN: usize = 128;

async fn pair_start(
    State(state): State<AppState>,
    Json(body): Json<PairStartRequest>,
) -> Result<Json<PairStartResponse>, ApiError> {
    if body.setup_token.len() > SETUP_TOKEN_MAX_LEN {
        return Err(ApiError::BadRequest("setup_token exceeds maximum length"));
    }

    let now = SystemTime::now();

    let user_handle = state
        .auth_store
        .transact(|s| {
            let (uh, next) = s.user_handle_or_init();
            Ok((next, uh))
        })
        .await?;

    let snap = state.auth_store.snapshot().await;
    let token = snap
        .find_redeemable_setup_token(&body.setup_token, now)
        .ok_or_else(|| {
            tracing::warn!("pair_start: setup token not redeemable");
            ApiError::Unauthorized
        })?;
    let setup_token_id = token.id.clone();

    let (challenge_id, ccr) = state
        .webauthn
        .start_registration(user_handle, "sipag", "Sipag", &snap.credentials, now)?;

    Ok(Json(PairStartResponse {
        challenge_id,
        setup_token_id,
        options: ccr,
    }))
}

async fn pair_finish(
    State(state): State<AppState>,
    Json(body): Json<PairFinishRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let now = SystemTime::now();
    let credential = state
        .webauthn
        .finish_paired_registration(
            &body.challenge_id,
            &body.response,
            body.setup_token_id.clone(),
            now,
        )
        .inspect_err(|e| {
            tracing::warn!(
                challenge_id = %body.challenge_id,
                error = %e,
                "pair ceremony failed"
            );
        })?;

    let new_credential = credential.clone();
    let setup_token_id = body.setup_token_id.clone();
    let cookie_secure = state.cookie_secure;
    let (plaintext_token, minted_session) = state
        .auth_store
        .transact(move |s| {
            let Some(token) = s.find_setup_token(&setup_token_id) else {
                return Err(AuthError::StateConflict(
                    "setup token revoked during pair ceremony",
                ));
            };
            if !token.is_redeemable(now) {
                return Err(AuthError::StateConflict(
                    "setup token no longer redeemable (consumed or expired)",
                ));
            }

            let (plaintext, session) =
                Session::mint(new_credential.id.clone(), now, SESSION_TTL);
            let next = s
                .upsert_credential(new_credential.clone())
                .consume_setup_token(&setup_token_id, &new_credential.id, now)
                .upsert_session(session.clone());
            Ok((next, (plaintext, session)))
        })
        .await?;

    tracing::info!(
        credential_id = %credential.id,
        setup_token_id = %body.setup_token_id,
        "paired new device; session minted"
    );

    Ok(session_cookie_response(
        StatusCode::CREATED,
        &plaintext_token,
        cookie_secure,
        Json(AuthFinishResponse {
            credential_id: credential.id,
            csrf_token: minted_session.csrf_token,
        }),
    ))
}

// ---------- logout ----------

async fn logout(
    State(state): State<AppState>,
    Authenticated(ctx): Authenticated,
) -> Result<impl IntoResponse, ApiError> {
    let AuthContext::Remote {
        plaintext_token,
        credential,
        ..
    } = ctx
    else {
        return Err(ApiError::Conflict("localhost peer has no session to end"));
    };

    state
        .auth_store
        .transact(move |s| Ok((s.remove_session(&plaintext_token), ())))
        .await?;

    let clear = build_clear_cookie(state.cookie_secure);
    tracing::info!(
        credential_id = %credential.id,
        "logout succeeded; session removed"
    );
    Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, clear)]))
}

// ---------- helpers ----------

fn session_cookie_response<B>(
    status: StatusCode,
    token: &str,
    secure: bool,
    body: B,
) -> impl IntoResponse
where
    B: IntoResponse,
{
    let cookie = build_set_cookie(token, SESSION_TTL.as_secs(), secure);
    (status, [(header::SET_COOKIE, cookie)], body)
}

fn require_localhost(peer: SocketAddr, headers: &HeaderMap) -> Result<(), ApiError> {
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    if matches!(AccessMethod::classify(peer, host), AccessMethod::Localhost) {
        Ok(())
    } else {
        tracing::warn!("rejecting remote request to first-device registration");
        Err(ApiError::Forbidden("forbidden"))
    }
}
