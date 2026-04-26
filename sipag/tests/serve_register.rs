//! Integration tests for the WebAuthn register ceremony.
//!
//! These cover the structural behavior of /api/auth/register/{begin,finish}
//! — token validation, state lookup, error paths. The crypto round-trip
//! (a real authenticator's response surviving finish) is tested
//! manually via the /login UI in #39; webauthn-rs itself owns its
//! correctness tests.

use axum_test::TestServer;
use sipag::serve::build_test_router;
use sipag_core::auth::{random_token, SetupPurpose, SetupToken};
use tempfile::TempDir;

fn server(sipag_dir: &TempDir) -> TestServer {
    let router = build_test_router(
        sipag_dir.path().to_path_buf(),
        "http://localhost:7100".to_string(),
    );
    TestServer::new(router).unwrap()
}

fn mint_setup_token(sipag_dir: &TempDir, ttl_minutes: i64) -> String {
    let token = random_token(16);
    SetupToken::new(token.clone(), SetupPurpose::EnrollPasskey, ttl_minutes)
        .save(sipag_dir.path())
        .unwrap();
    token
}

#[tokio::test]
async fn register_begin_with_valid_setup_token_returns_state_id_and_challenge() {
    let dir = TempDir::new().unwrap();
    let token = mint_setup_token(&dir, 10);

    let response = server(&dir)
        .post("/api/auth/register/begin")
        .json(&serde_json::json!({ "setup_token": token }))
        .await;

    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert!(
        body.get("state_id")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty()),
        "expected non-empty state_id; got {body:#}"
    );
    let challenge = body.get("challenge").expect("challenge missing");
    // CreationChallengeResponse top level: { publicKey: {...} }
    assert!(
        challenge.get("publicKey").is_some(),
        "expected challenge.publicKey shape; got {challenge:#}"
    );
}

#[tokio::test]
async fn register_begin_without_setup_token_returns_400() {
    let dir = TempDir::new().unwrap();
    let response = server(&dir)
        .post("/api/auth/register/begin")
        .json(&serde_json::json!({ "setup_token": "" }))
        .await;
    response.assert_status_bad_request();
}

#[tokio::test]
async fn register_begin_with_unknown_setup_token_returns_404() {
    let dir = TempDir::new().unwrap();
    let response = server(&dir)
        .post("/api/auth/register/begin")
        .json(&serde_json::json!({ "setup_token": "no-such-token" }))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn register_begin_with_expired_token_returns_410() {
    let dir = TempDir::new().unwrap();
    let token = mint_setup_token(&dir, -1);
    let response = server(&dir)
        .post("/api/auth/register/begin")
        .json(&serde_json::json!({ "setup_token": token }))
        .await;
    response.assert_status(axum::http::StatusCode::GONE);
}

#[tokio::test]
async fn register_begin_with_wrong_purpose_returns_410() {
    let dir = TempDir::new().unwrap();
    let token = random_token(16);
    SetupToken::new(token.clone(), SetupPurpose::KatulongAppInstall, 10)
        .save(dir.path())
        .unwrap();

    let response = server(&dir)
        .post("/api/auth/register/begin")
        .json(&serde_json::json!({ "setup_token": token }))
        .await;
    response.assert_status(axum::http::StatusCode::GONE);
}

#[tokio::test]
async fn register_finish_with_unknown_state_returns_404() {
    let dir = TempDir::new().unwrap();
    let response = server(&dir)
        .post("/api/auth/register/finish")
        .json(&serde_json::json!({
            "state_id": "no-such-state",
            "credential": {
                "id": "AAA",
                "rawId": "AAA",
                "type": "public-key",
                "response": {
                    "attestationObject": "AAA",
                    "clientDataJSON": "AAA",
                },
                "extensions": {},
            },
            "label": "test",
        }))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn register_finish_with_malformed_credential_returns_422() {
    let dir = TempDir::new().unwrap();
    // Send a body where `credential` doesn't deserialize into
    // RegisterPublicKeyCredential — axum's Json extractor rejects
    // before our handler runs.
    let response = server(&dir)
        .post("/api/auth/register/finish")
        .json(&serde_json::json!({
            "state_id": "some-state",
            "credential": "not an object",
            "label": "",
        }))
        .await;
    // Json extractor returns 422 on shape mismatch.
    assert!(
        response.status_code() == axum::http::StatusCode::UNPROCESSABLE_ENTITY
            || response.status_code() == axum::http::StatusCode::BAD_REQUEST,
        "expected 422 or 400 on malformed credential; got {}",
        response.status_code()
    );
}
