//! Async helpers for talking to a katulong host from sipag's HTTP
//! handlers. Sits at the layer where reqwest meets axum: takes a
//! reqwest::Client, returns axum-shaped (StatusCode, String) errors.
//!
//! The wire format itself (URLs, JSON shapes) lives in
//! `sipag_core::katulong`. This module just orchestrates the
//! request/response shape into something axum and htmx handlers can
//! return directly.

use axum::http::StatusCode;
use tracing::warn;

/// Parse `POST /sessions` response as `sipag_core::katulong::Session`
/// and return the session id. Returns a (status, body) pair on parse
/// failure so callers can adapt to their preferred response idiom.
async fn extract_session_id(
    resp: reqwest::Response,
    host_id: &str,
) -> std::result::Result<String, (StatusCode, String)> {
    match resp.json::<sipag_core::katulong::Session>().await {
        Ok(s) => Ok(s.id),
        Err(e) => {
            warn!(host = %host_id, error = %e, "parse session create response failed");
            Err((
                StatusCode::BAD_GATEWAY,
                format!("create session on {host_id}: invalid response: {e}"),
            ))
        }
    }
}

/// Idempotent create-or-find by session name. POSTs `/sessions` and
/// returns the new session's id; on 409 (already exists) falls back
/// to `GET /sessions` and finds by name. Mirrors the behavior of
/// `KatulongClient::create_session` in sipag-core for the
/// reqwest/async transport that serve/ uses, so the dispatch path is
/// idempotent across both transports. Returns a ready-to-respond
/// (status, body) pair on any failure.
pub(super) async fn create_or_find_session(
    http: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    host_id: &str,
    name: &str,
) -> std::result::Result<String, (StatusCode, String)> {
    let url = sipag_core::katulong::sessions_url(base_url);
    let create_resp = match http
        .post(&url)
        .bearer_auth(api_key)
        .json(&serde_json::json!({ "name": name }))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            // `error = %e` carries the request URL via reqwest's
            // Display impl — keep it in the warn log only, never in
            // the response body (would leak the tunnel hostname).
            warn!(host = %host_id, error = %e, "POST /sessions failed");
            return Err((
                StatusCode::BAD_GATEWAY,
                format!("create session on {host_id}: network error"),
            ));
        }
    };

    match create_resp.status().as_u16() {
        200 | 201 => extract_session_id(create_resp, host_id).await,
        409 => {
            // Already exists. List and find by name — same recovery
            // path as KatulongClient::create_session.
            let list_resp = match http.get(&url).bearer_auth(api_key).send().await {
                Ok(r) => r,
                Err(e) => {
                    warn!(host = %host_id, error = %e, "GET /sessions for 409 fallback failed");
                    return Err((
                        StatusCode::BAD_GATEWAY,
                        format!("create session on {host_id}: network error"),
                    ));
                }
            };
            if !list_resp.status().is_success() {
                let st = list_resp.status();
                let body = list_resp.text().await.unwrap_or_default();
                return Err((
                    StatusCode::BAD_GATEWAY,
                    format!(
                        "create session on {host_id}: 409 fallback list failed: HTTP {st}: {body}"
                    ),
                ));
            }
            let sessions: Vec<sipag_core::katulong::Session> = match list_resp.json().await {
                Ok(s) => s,
                Err(e) => {
                    warn!(host = %host_id, error = %e, "parse /sessions list failed");
                    return Err((
                        StatusCode::BAD_GATEWAY,
                        format!("create session on {host_id}: invalid list response"),
                    ));
                }
            };
            sessions
                .into_iter()
                .find(|s| s.name == name)
                .map(|s| s.id)
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_GATEWAY,
                        format!(
                            "session '{name}' on {host_id} returned 409 but list lookup missed it"
                        ),
                    )
                })
        }
        code => {
            let body = create_resp.text().await.unwrap_or_default();
            Err((
                StatusCode::BAD_GATEWAY,
                format!("create session on {host_id}: HTTP {code}: {body}"),
            ))
        }
    }
}
