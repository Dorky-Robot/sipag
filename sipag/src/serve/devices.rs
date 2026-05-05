//! Device (credential) management routes.
//!
//! - `GET    /api/auth/devices`      — list with per-device metadata
//! - `DELETE /api/auth/devices/:id`  — remove credential + sessions;
//!   blocked for remote callers if it's the last credential
//!
//! The last-credential guard applies only to remote callers. A localhost
//! caller has physical access regardless — locking them out via
//! "delete last passkey" would be a false safety.

use crate::serve::auth_middleware::{AuthContext, Authenticated};
use crate::serve::error::ApiError;
use crate::serve::state::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get},
    Json, Router,
};
use serde::Serialize;
use std::time::SystemTime;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/auth/devices", get(list_devices))
        .route("/api/auth/devices/:id", delete(revoke_device))
}

#[derive(Debug, Serialize)]
struct DeviceEntry {
    id: String,
    name: Option<String>,
    created_at_millis: u64,
    counter: u32,
    setup_token_id: Option<String>,
    is_current: bool,
}

async fn list_devices(
    State(state): State<AppState>,
    Authenticated(ctx): Authenticated,
) -> Result<Json<Vec<DeviceEntry>>, ApiError> {
    let current_id = match &ctx {
        AuthContext::Remote { credential, .. } => Some(credential.id.clone()),
        AuthContext::Localhost => None,
    };
    let snap = state.auth_store.snapshot().await;
    let entries: Vec<DeviceEntry> = snap
        .credentials
        .iter()
        .map(|c| DeviceEntry {
            id: c.id.clone(),
            name: c.name.clone(),
            created_at_millis: c
                .created_at
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            counter: c.counter,
            setup_token_id: c.setup_token_id.clone(),
            is_current: current_id.as_deref() == Some(c.id.as_str()),
        })
        .collect();
    Ok(Json(entries))
}

async fn revoke_device(
    State(state): State<AppState>,
    Authenticated(ctx): Authenticated,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let is_localhost = matches!(ctx, AuthContext::Localhost);
    let revoked_by = match &ctx {
        AuthContext::Localhost => "localhost".to_string(),
        AuthContext::Remote { credential, .. } => credential.id.clone(),
    };

    let credential_id = id.clone();
    let _existed = state
        .auth_store
        .transact(move |s| {
            let target_exists = s.find_credential(&id).is_some();
            if !is_localhost && target_exists && s.credentials.len() == 1 {
                return Err(sipag_core::auth::AuthError::LastCredentialRemoval);
            }
            Ok((s.remove_credential(&id), target_exists))
        })
        .await?;

    tracing::info!(
        credential_id = %credential_id,
        revoked_by = %revoked_by,
        "device revoked"
    );
    Ok(StatusCode::NO_CONTENT)
}
