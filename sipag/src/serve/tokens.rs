//! Setup-token management routes.
//!
//! Auth-protected. The currently-signed-in user mints tokens and hands
//! the plaintext to the new device's operator. The new device hits
//! `/api/auth/pair/{start,finish}` (in `auth.rs`), which consumes the
//! token and registers a credential bidirectionally linked to it.
//!
//! Three endpoints:
//! - `GET    /api/auth/setup-tokens`       — list
//! - `POST   /api/auth/setup-tokens`       — create
//! - `DELETE /api/auth/setup-tokens/:id`   — revoke
//!
//! Revocation cascades to the paired device AND its sessions.

use crate::serve::auth_middleware::Authenticated;
use crate::serve::error::ApiError;
use crate::serve::state::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sipag_core::auth::{PlaintextToken, SetupToken};
use std::time::{Duration, SystemTime};

/// Default setup-token lifetime. One hour is long enough to copy the
/// plaintext to another device and short enough that a lost token
/// doesn't sit redeemable overnight.
const SETUP_TOKEN_TTL: Duration = Duration::from_secs(60 * 60);

/// Max length for the human-readable `name` field.
const TOKEN_NAME_MAX_LEN: usize = 64;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/auth/setup-tokens",
            get(list_tokens).post(create_token),
        )
        .route("/api/auth/setup-tokens/:id", delete(revoke_token))
}

#[derive(Debug, Serialize)]
struct TokenListEntry {
    id: String,
    name: Option<String>,
    expires_at_millis: u64,
    /// `"live"` | `"used"` | `"expired"`
    status: &'static str,
    credential_id: Option<String>,
}

async fn list_tokens(
    State(state): State<AppState>,
    Authenticated(_): Authenticated,
) -> Result<Json<Vec<TokenListEntry>>, ApiError> {
    let now = SystemTime::now();
    let snap = state.auth_store.snapshot().await;
    let entries: Vec<TokenListEntry> = snap
        .setup_tokens
        .iter()
        .map(|t| TokenListEntry {
            id: t.id.clone(),
            name: t.name.clone(),
            expires_at_millis: t
                .expires_at
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            status: if t.is_consumed() {
                "used"
            } else if t.is_expired(now) {
                "expired"
            } else {
                "live"
            },
            credential_id: t.credential_id.clone(),
        })
        .collect();
    Ok(Json(entries))
}

#[derive(Debug, Deserialize)]
struct CreateTokenRequest {
    name: Option<String>,
}

#[derive(Debug, Serialize)]
struct CreateTokenResponse {
    id: String,
    /// Hand this to the user. Once the response is sent, the server
    /// holds only the scrypt hash — recovery is impossible.
    plaintext: PlaintextToken,
    expires_at_millis: u64,
}

async fn create_token(
    State(state): State<AppState>,
    Authenticated(_): Authenticated,
    Json(body): Json<CreateTokenRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let name = body
        .name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty());
    if let Some(ref n) = name {
        if n.chars().count() > TOKEN_NAME_MAX_LEN {
            return Err(ApiError::BadRequest("name exceeds 64 characters"));
        }
    }
    let now = SystemTime::now();
    let (plaintext, token) = SetupToken::issue(name, now, SETUP_TOKEN_TTL)?;
    let id = token.id.clone();
    let expires_at = token.expires_at;
    state
        .auth_store
        .transact(|s| Ok((s.add_setup_token(token.clone()), ())))
        .await?;

    tracing::info!(token_id = %id, "setup token minted");
    Ok((
        StatusCode::CREATED,
        Json(CreateTokenResponse {
            id,
            plaintext,
            expires_at_millis: expires_at
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        }),
    ))
}

async fn revoke_token(
    State(state): State<AppState>,
    Authenticated(_): Authenticated,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let token_id = id.clone();
    let paired_credential_id = state
        .auth_store
        .transact(move |s| {
            let paired = s
                .find_setup_token(&id)
                .and_then(|t| t.credential_id.clone());
            Ok((s.remove_setup_token(&id), paired))
        })
        .await?;

    match paired_credential_id {
        Some(cred_id) => tracing::info!(
            token_id = %token_id,
            credential_id = %cred_id,
            "setup token revoked; paired credential removed"
        ),
        None => tracing::info!(
            token_id = %token_id,
            "setup token revoked (no paired credential)"
        ),
    }
    Ok(StatusCode::NO_CONTENT)
}
