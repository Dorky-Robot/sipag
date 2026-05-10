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

/// Cap on how many chars of an upstream katulong error body we
/// echo back to the caller. Long enough for a useful one-line
/// JSON error, short enough that pathological responses (large
/// stack traces, attacker-supplied content) can't bloat sipag's
/// own response or disrupt log forwarders. Char-bounded, not
/// byte-bounded — see [`sanitize_upstream_body`].
const UPSTREAM_BODY_MAX_CHARS: usize = 256;

/// Sanitize a katulong response body before interpolating it into
/// sipag's own **plain-text** response (axum tuple body, htmx error
/// string). Strips ASCII control characters (Rust's
/// [`char::is_control`] — covers C0 + DEL + C1) except `\n` and
/// `\t` so multi-line JSON errors stay readable, and truncates to
/// [`UPSTREAM_BODY_MAX_CHARS`] chars.
///
/// Caller responsibilities:
/// - **HTML contexts**: do NOT pass sanitized output to a `maud`
///   fragment or other HTML renderer. The filter strips terminal
///   escapes and bytes that disrupt log lines, but does not encode
///   `<`, `>`, `&`, or strip Unicode bidi-override chars
///   (U+202A-202E etc.) that can disrupt visual layout. For HTML
///   error paths, drop body forwarding entirely and rely on the
///   `warn!` log instead — see `observation_transcript_handler`.
/// - **Operator visibility**: the full body should be `warn!`-logged
///   *before* this sanitizer runs so operators retain the raw
///   diagnostic text. Sanitization governs only what crosses to the
///   HTTP caller.
pub(super) fn sanitize_upstream_body(body: &str) -> String {
    body.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .take(UPSTREAM_BODY_MAX_CHARS)
        .collect()
}

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
                let raw = list_resp.text().await.unwrap_or_default();
                warn!(host = %host_id, status = %st, body = %raw, "GET /sessions for 409 fallback returned non-2xx");
                let body = sanitize_upstream_body(&raw);
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
            let raw = create_resp.text().await.unwrap_or_default();
            warn!(host = %host_id, status = code, body = %raw, "POST /sessions returned non-2xx");
            let body = sanitize_upstream_body(&raw);
            Err((
                StatusCode::BAD_GATEWAY,
                format!("create session on {host_id}: HTTP {code}: {body}"),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_passes_short_well_formed_bodies() {
        assert_eq!(
            sanitize_upstream_body(r#"{"error":"Session already exists"}"#),
            r#"{"error":"Session already exists"}"#
        );
    }

    #[test]
    fn sanitize_keeps_newlines_and_tabs() {
        // multi-line JSON errors stay readable
        let s = "line one\nline two\twith tab";
        assert_eq!(sanitize_upstream_body(s), s);
    }

    #[test]
    fn sanitize_strips_other_control_chars() {
        // \r, NUL, ESC, BEL — anything that could disrupt a log line
        // or terminal rendering — is dropped.
        let s = "before\rafter\0nul\x1bescape\x07bell";
        assert_eq!(sanitize_upstream_body(s), "beforeafternulescapebell");
    }

    #[test]
    fn sanitize_truncates_to_cap() {
        let s: String = "x".repeat(UPSTREAM_BODY_MAX_CHARS + 100);
        let out = sanitize_upstream_body(&s);
        // Assert char-count, not byte-len — the cap is char-bounded.
        // For ASCII these are equal, but the doc contract is chars.
        assert_eq!(out.chars().count(), UPSTREAM_BODY_MAX_CHARS);
        assert_eq!(out.len(), UPSTREAM_BODY_MAX_CHARS); // ASCII: bytes == chars
    }

    #[test]
    fn sanitize_truncates_to_cap_for_multibyte_chars() {
        // 4-byte UTF-8 chars (U+1F600 grinning face) cap at chars,
        // not bytes — the output should be UPSTREAM_BODY_MAX_CHARS
        // chars × 4 bytes, not UPSTREAM_BODY_MAX_CHARS bytes.
        let s: String = "😀".repeat(UPSTREAM_BODY_MAX_CHARS + 50);
        let out = sanitize_upstream_body(&s);
        assert_eq!(out.chars().count(), UPSTREAM_BODY_MAX_CHARS);
        assert_eq!(out.len(), UPSTREAM_BODY_MAX_CHARS * 4);
    }

    #[test]
    fn sanitize_truncates_after_filtering() {
        // Control chars are filtered first, so they don't count
        // against the cap. (Reversed order would leave the output
        // shorter than UPSTREAM_BODY_MAX_CHARS.)
        let mut s = String::new();
        for _ in 0..50 {
            s.push('\r');
        }
        for _ in 0..UPSTREAM_BODY_MAX_CHARS {
            s.push('x');
        }
        assert_eq!(
            sanitize_upstream_body(&s).chars().count(),
            UPSTREAM_BODY_MAX_CHARS
        );
    }
}
