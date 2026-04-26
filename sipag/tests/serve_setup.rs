//! Integration tests for the `/setup` endpoint — the gate that decides
//! whether a setup-token URL renders the register-passkey page.
//!
//! Each test owns its own tempdir + AppState, so they run in parallel
//! without env-var contention.

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

fn mint_token(sipag_dir: &TempDir, purpose: SetupPurpose, ttl_minutes: i64) -> String {
    let token = random_token(16);
    SetupToken::new(token.clone(), purpose, ttl_minutes)
        .save(sipag_dir.path())
        .unwrap();
    token
}

#[tokio::test]
async fn get_setup_with_valid_token_renders_register_page() {
    let dir = TempDir::new().unwrap();
    let token = mint_token(&dir, SetupPurpose::EnrollPasskey, 10);

    let response = server(&dir).get(&format!("/setup?token={}", token)).await;

    response.assert_status_ok();
    let body = response.text();
    assert!(
        body.contains("Register your passkey"),
        "expected register heading in body; got:\n{body}"
    );
    assert!(
        body.contains(&token[..8]),
        "expected token (or prefix) embedded in body so JS can post it back"
    );
}

#[tokio::test]
async fn get_setup_with_unknown_token_returns_404() {
    let dir = TempDir::new().unwrap();
    let response = server(&dir).get("/setup?token=does-not-exist").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn get_setup_with_expired_token_returns_410() {
    let dir = TempDir::new().unwrap();
    // Negative TTL = already expired.
    let token = mint_token(&dir, SetupPurpose::EnrollPasskey, -1);
    let response = server(&dir).get(&format!("/setup?token={}", token)).await;
    response.assert_status(axum::http::StatusCode::GONE);
}

#[tokio::test]
async fn get_setup_with_no_token_param_returns_404() {
    let dir = TempDir::new().unwrap();
    let response = server(&dir).get("/setup").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn get_setup_with_wrong_purpose_returns_410() {
    let dir = TempDir::new().unwrap();
    // Token minted for the install handshake, not for enrollment.
    let token = mint_token(&dir, SetupPurpose::KatulongAppInstall, 10);
    let response = server(&dir).get(&format!("/setup?token={}", token)).await;
    response.assert_status(axum::http::StatusCode::GONE);
}

#[tokio::test]
async fn get_setup_does_not_consume_the_token() {
    // The token should remain valid until register/finish actually
    // burns it. A user refreshing the page mid-enrollment shouldn't
    // lose their bootstrap.
    let dir = TempDir::new().unwrap();
    let token = mint_token(&dir, SetupPurpose::EnrollPasskey, 10);

    let s = server(&dir);
    s.get(&format!("/setup?token={}", token))
        .await
        .assert_status_ok();
    s.get(&format!("/setup?token={}", token))
        .await
        .assert_status_ok();

    // Token is still on disk.
    let still_loadable = SetupToken::load(dir.path(), &token);
    assert!(
        still_loadable.is_ok(),
        "GET /setup must not consume the token"
    );
}
