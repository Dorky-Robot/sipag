//! Integration tests for the /login page — branches on whether any
//! credentials are enrolled.

use axum_test::TestServer;
use sipag::serve::build_test_router;
use sipag_core::auth::Credential;
use tempfile::TempDir;

fn server(sipag_dir: &TempDir) -> TestServer {
    let router = build_test_router(
        sipag_dir.path().to_path_buf(),
        "http://localhost:7100".to_string(),
    );
    TestServer::new(router).unwrap()
}

fn write_dummy_credential(sipag_dir: &TempDir) {
    // Bypass the WebAuthn ceremony for testing — write a minimal
    // Credential record by hand. The /login UI only needs to know
    // "is the store empty or not", not the passkey contents.
    Credential {
        id: "dummy-cred".into(),
        label: "fixture".into(),
        created: "2026-04-26T00:00:00Z".into(),
        last_used: None,
        passkey: serde_json::json!({ "stub": true }),
    }
    .save(sipag_dir.path())
    .unwrap();
}

#[tokio::test]
async fn login_with_no_credentials_directs_user_to_setup() {
    let dir = TempDir::new().unwrap();
    let response = server(&dir).get("/login").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(
        body.contains("sipag setup-token"),
        "empty-store login page should mention `sipag setup-token`; got:\n{body}"
    );
}

#[tokio::test]
async fn login_with_credentials_renders_signin_button() {
    let dir = TempDir::new().unwrap();
    write_dummy_credential(&dir);
    let response = server(&dir).get("/login").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(
        body.to_lowercase().contains("sign in"),
        "populated-store login page should offer sign-in; got:\n{body}"
    );
    assert!(
        body.contains("/api/auth/assert/begin"),
        "page should reference assert/begin endpoint; got:\n{body}"
    );
}

#[tokio::test]
async fn login_returns_html_content_type() {
    let dir = TempDir::new().unwrap();
    let response = server(&dir).get("/login").await;
    let ct = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        ct.starts_with("text/html"),
        "expected text/html content-type; got {ct:?}"
    );
}
