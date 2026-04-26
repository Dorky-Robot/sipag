//! Integration tests for the WebAuthn assert ceremony + logout.
//!
//! Same structural-only depth as serve_register.rs. The crypto
//! round-trip lands in #39's manual smoke test.

use axum_test::TestServer;
use sipag::serve::build_test_router;
use tempfile::TempDir;

fn server(sipag_dir: &TempDir) -> TestServer {
    let router = build_test_router(
        sipag_dir.path().to_path_buf(),
        "http://localhost:7100".to_string(),
    );
    TestServer::new(router).unwrap()
}

#[tokio::test]
async fn assert_begin_with_no_credentials_returns_404() {
    let dir = TempDir::new().unwrap();
    let response = server(&dir).post("/api/auth/assert/begin").await;
    response.assert_status_not_found();
    let body: serde_json::Value = response.json();
    assert_eq!(body["error"], "credential.none");
}

#[tokio::test]
async fn assert_finish_with_unknown_state_returns_404() {
    let dir = TempDir::new().unwrap();
    let response = server(&dir)
        .post("/api/auth/assert/finish")
        .json(&serde_json::json!({
            "state_id": "no-such-state",
            "credential": {
                "id": "AAA",
                "rawId": "AAA",
                "type": "public-key",
                "response": {
                    "authenticatorData": "AAA",
                    "clientDataJSON": "AAA",
                    "signature": "AAA",
                },
                "extensions": {},
            },
        }))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn logout_without_session_cookie_is_idempotent() {
    let dir = TempDir::new().unwrap();
    let response = server(&dir).post("/api/auth/logout").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["ok"], true);
}

#[tokio::test]
async fn assert_finish_with_malformed_credential_returns_422() {
    let dir = TempDir::new().unwrap();
    let response = server(&dir)
        .post("/api/auth/assert/finish")
        .json(&serde_json::json!({
            "state_id": "abc",
            "credential": 42,
        }))
        .await;
    assert!(
        response.status_code() == axum::http::StatusCode::UNPROCESSABLE_ENTITY
            || response.status_code() == axum::http::StatusCode::BAD_REQUEST,
        "expected 422 or 400; got {}",
        response.status_code()
    );
}
