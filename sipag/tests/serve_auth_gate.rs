//! Integration tests for the auth middleware:
//!   - localhost Host header bypasses auth (dev convenience)
//!   - non-localhost Host requires a session cookie or returns
//!     401 (for /api/*) or 302 → /login (for SPA shell)
//!   - public paths (/login, /setup, /api/auth/*, /js/*, /style.css)
//!     are always reachable

use axum_test::TestServer;
use sipag::serve::build_test_router;
use tempfile::TempDir;

const TUNNEL_HOST: &str = "sipag.felixflor.es";

fn server(sipag_dir: &TempDir) -> TestServer {
    let router = build_test_router(
        sipag_dir.path().to_path_buf(),
        format!("https://{TUNNEL_HOST}"),
    );
    TestServer::new(router).unwrap()
}

#[tokio::test]
async fn localhost_host_bypasses_auth_for_api() {
    let dir = TempDir::new().unwrap();
    let response = server(&dir)
        .get("/api/projects")
        .add_header("host", "localhost:7100")
        .await;
    response.assert_status_ok();
}

#[tokio::test]
async fn loopback_ip_bypasses_auth_for_api() {
    let dir = TempDir::new().unwrap();
    let response = server(&dir)
        .get("/api/projects")
        .add_header("host", "127.0.0.1:7100")
        .await;
    response.assert_status_ok();
}

#[tokio::test]
async fn tunnel_host_without_cookie_returns_401_for_api() {
    let dir = TempDir::new().unwrap();
    let response = server(&dir)
        .get("/api/projects")
        .add_header("host", TUNNEL_HOST)
        .await;
    response.assert_status(axum::http::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn tunnel_host_without_cookie_redirects_html_to_login() {
    let dir = TempDir::new().unwrap();
    let response = server(&dir)
        .get("/")
        .add_header("host", TUNNEL_HOST)
        .add_header("accept", "text/html")
        .await;
    let status = response.status_code();
    assert!(
        status == axum::http::StatusCode::FOUND
            || status == axum::http::StatusCode::SEE_OTHER
            || status == axum::http::StatusCode::TEMPORARY_REDIRECT,
        "expected 302/303/307; got {status}"
    );
    let location = response
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert_eq!(location, "/login");
}

#[tokio::test]
async fn login_is_publicly_reachable_from_tunnel_host() {
    let dir = TempDir::new().unwrap();
    let response = server(&dir)
        .get("/login")
        .add_header("host", TUNNEL_HOST)
        .await;
    response.assert_status_ok();
}

#[tokio::test]
async fn setup_is_publicly_reachable_from_tunnel_host() {
    let dir = TempDir::new().unwrap();
    let response = server(&dir)
        .get("/setup?token=anything")
        .add_header("host", TUNNEL_HOST)
        .await;
    // 404 because the token doesn't exist, but we DID reach the
    // handler — the auth gate didn't block us.
    response.assert_status_not_found();
}

#[tokio::test]
async fn auth_endpoints_publicly_reachable_from_tunnel_host() {
    let dir = TempDir::new().unwrap();
    // Without auth a register/begin reaches the handler and rejects
    // with 400 (missing setup_token) — proving the gate didn't fire.
    let response = server(&dir)
        .post("/api/auth/register/begin")
        .add_header("host", TUNNEL_HOST)
        .json(&serde_json::json!({ "setup_token": "" }))
        .await;
    // The auth gate would return 401 here if it were misapplied;
    // the handler returns 400 for empty setup_token.
    response.assert_status_bad_request();
}

#[tokio::test]
async fn assert_endpoints_publicly_reachable_from_tunnel_host() {
    let dir = TempDir::new().unwrap();
    let response = server(&dir)
        .post("/api/auth/assert/begin")
        .add_header("host", TUNNEL_HOST)
        .await;
    // No credentials enrolled → 404 from the handler. Auth gate
    // didn't intercept (would have been 401).
    response.assert_status_not_found();
}

#[tokio::test]
async fn valid_session_cookie_unlocks_api_from_tunnel_host() {
    use sipag_core::auth::Session;
    let dir = TempDir::new().unwrap();
    // Hand-mint a valid session record (skipping the WebAuthn
    // ceremony) so we can test the cookie-validation path.
    let session = Session::new("test-token".into(), "dummy-cred".into());
    session.save(dir.path()).unwrap();

    let response = server(&dir)
        .get("/api/projects")
        .add_header("host", TUNNEL_HOST)
        .add_header("cookie", "sipag_session=test-token")
        .await;
    response.assert_status_ok();
}

#[tokio::test]
async fn expired_session_cookie_returns_401() {
    use sipag_core::auth::Session;
    let dir = TempDir::new().unwrap();
    let mut session = Session::new("stale-token".into(), "dummy-cred".into());
    // Force expiry into the past.
    session.expires = "2020-01-01T00:00:00Z".into();
    session.save(dir.path()).unwrap();

    let response = server(&dir)
        .get("/api/projects")
        .add_header("host", TUNNEL_HOST)
        .add_header("cookie", "sipag_session=stale-token")
        .await;
    response.assert_status(axum::http::StatusCode::UNAUTHORIZED);
}
