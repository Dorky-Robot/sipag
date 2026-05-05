//! Integration tests for the post-katulong-port auth surface:
//!
//! - the gate (localhost bypass, tunnel cookie requirement, public paths)
//! - register/start being localhost-only
//! - login/start emitting 409 when the install is fresh
//! - /api/auth/status reflecting access mode + has_credentials
//!
//! WebAuthn ceremony round-trips need a soft authenticator that we
//! don't have wired up yet — we test our boundaries (gate, route
//! reachability, error shapes), and trust webauthn-rs's own test suite
//! for the crypto path.

use axum::{http::StatusCode, Router};
use axum_test::{TestServer, TestServerConfig, Transport};
use serde_json::json;
use sipag::serve::{build_test_router, build_test_state, AppState};
use sipag_core::auth::{Credential, Session, SESSION_TTL};
use std::time::SystemTime;
use tempfile::TempDir;

const TUNNEL_HOST: &str = "sipag.felixflor.es";

async fn router(sipag_dir: &TempDir, public_url: &str) -> (Router, AppState) {
    let state = build_test_state(sipag_dir.path().to_path_buf(), public_url.to_string())
        .await
        .expect("build_test_state");
    let web_root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let router = build_test_router(state.clone(), web_root);
    (router, state)
}

async fn server(sipag_dir: &TempDir) -> (TestServer, AppState) {
    let (router, state) = router(sipag_dir, &format!("https://{TUNNEL_HOST}")).await;
    // Real HTTP on a random port — sets up ConnectInfo<SocketAddr>
    // (loopback peer) which the gate middleware extracts. The default
    // mock transport doesn't wire ConnectInfo, so requests there 500
    // before the gate can decide.
    let cfg = TestServerConfig {
        transport: Some(Transport::HttpRandomPort),
        ..TestServerConfig::default()
    };
    let server = TestServer::new_with_config(router, cfg).expect("TestServer");
    (server, state)
}

// Seed a credential + session record so the cookie-validated path has
// something to find. Bypasses the WebAuthn ceremony — we just need a
// row to look up.
async fn seed_session(state: &AppState, plaintext_token: &str) {
    let now = SystemTime::now();
    let cred = Credential {
        id: "test-cred".into(),
        public_key: vec![1, 2, 3],
        name: Some("test".into()),
        counter: 0,
        created_at: now,
        setup_token_id: None,
    };

    let token_owned = plaintext_token.to_string();
    let cred_id = cred.id.clone();
    state
        .auth_store
        .transact(move |s| {
            let session = Session {
                token_hash: sha256_hex(&token_owned),
                credential_id: cred_id,
                csrf_token: "csrf".into(),
                created_at: now,
                expires_at: now + SESSION_TTL,
                last_activity_at: now,
            };
            Ok((s.upsert_credential(cred).upsert_session(session), ()))
        })
        .await
        .unwrap();
}

fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    let digest = Sha256::new().chain_update(s.as_bytes()).finalize();
    let mut out = String::with_capacity(64);
    for b in digest.iter() {
        write!(&mut out, "{b:02x}").unwrap();
    }
    out
}

// ---------- gate ----------

#[tokio::test]
async fn localhost_host_bypasses_auth_for_api() {
    let dir = TempDir::new().unwrap();
    let (server, _) = server(&dir).await;
    let response = server
        .get("/api/projects")
        .add_header("host", "localhost:7100")
        .await;
    response.assert_status_ok();
}

#[tokio::test]
async fn loopback_ip_bypasses_auth_for_api() {
    let dir = TempDir::new().unwrap();
    let (server, _) = server(&dir).await;
    let response = server
        .get("/api/projects")
        .add_header("host", "127.0.0.1:7100")
        .await;
    response.assert_status_ok();
}

#[tokio::test]
async fn tunnel_host_without_cookie_returns_401_for_api() {
    let dir = TempDir::new().unwrap();
    let (server, _) = server(&dir).await;
    let response = server
        .get("/api/projects")
        .add_header("host", TUNNEL_HOST)
        .await;
    response.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn tunnel_host_without_cookie_redirects_html_to_login() {
    let dir = TempDir::new().unwrap();
    let (server, _) = server(&dir).await;
    let response = server
        .get("/")
        .add_header("host", TUNNEL_HOST)
        .add_header("accept", "text/html")
        .await;
    let status = response.status_code();
    assert!(
        status == StatusCode::FOUND
            || status == StatusCode::SEE_OTHER
            || status == StatusCode::TEMPORARY_REDIRECT,
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
    let (server, _) = server(&dir).await;
    let response = server.get("/login").add_header("host", TUNNEL_HOST).await;
    response.assert_status_ok();
}

#[tokio::test]
async fn auth_endpoints_publicly_reachable_from_tunnel_host() {
    let dir = TempDir::new().unwrap();
    let (server, _) = server(&dir).await;
    // login/start with no credentials → 409 from the handler. Auth
    // gate would have intercepted with 401 — proving it didn't fire.
    let response = server
        .post("/api/auth/login/start")
        .add_header("host", TUNNEL_HOST)
        .await;
    response.assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn valid_session_cookie_unlocks_api_from_tunnel_host() {
    let dir = TempDir::new().unwrap();
    let (server, state) = server(&dir).await;
    seed_session(&state, "test-token").await;

    let response = server
        .get("/api/projects")
        .add_header("host", TUNNEL_HOST)
        .add_header("cookie", "sipag_session=test-token")
        .await;
    response.assert_status_ok();
}

#[tokio::test]
async fn unknown_session_cookie_returns_401() {
    let dir = TempDir::new().unwrap();
    let (server, _) = server(&dir).await;

    let response = server
        .get("/api/projects")
        .add_header("host", TUNNEL_HOST)
        .add_header("cookie", "sipag_session=stale-token")
        .await;
    response.assert_status(StatusCode::UNAUTHORIZED);
}

// ---------- /api/auth/status ----------

#[tokio::test]
async fn status_reports_no_credentials_on_fresh_install() {
    let dir = TempDir::new().unwrap();
    let (server, _) = server(&dir).await;

    let response = server
        .get("/api/auth/status")
        .add_header("host", TUNNEL_HOST)
        .await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["has_credentials"], json!(false));
    assert_eq!(body["access_method"], json!("remote"));
    assert_eq!(body["authenticated"], json!(false));
}

#[tokio::test]
async fn status_reports_localhost_when_host_is_loopback() {
    let dir = TempDir::new().unwrap();
    let (server, _) = server(&dir).await;

    let response = server
        .get("/api/auth/status")
        .add_header("host", "localhost:7100")
        .await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["access_method"], json!("localhost"));
}

#[tokio::test]
async fn status_reports_has_credentials_after_seed() {
    let dir = TempDir::new().unwrap();
    let (server, state) = server(&dir).await;
    seed_session(&state, "tk").await;

    let response = server
        .get("/api/auth/status")
        .add_header("host", TUNNEL_HOST)
        .await;
    let body: serde_json::Value = response.json();
    assert_eq!(body["has_credentials"], json!(true));
}

// ---------- register / login error shapes ----------

#[tokio::test]
async fn register_start_rejects_remote_caller() {
    let dir = TempDir::new().unwrap();
    let (server, _) = server(&dir).await;
    let response = server
        .post("/api/auth/register/start")
        .add_header("host", TUNNEL_HOST)
        .await;
    response.assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn login_start_409_on_fresh_install() {
    let dir = TempDir::new().unwrap();
    let (server, _) = server(&dir).await;
    let response = server
        .post("/api/auth/login/start")
        .add_header("host", "localhost:7100")
        .await;
    response.assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn pair_start_with_unknown_token_returns_401() {
    let dir = TempDir::new().unwrap();
    let (server, _) = server(&dir).await;
    let response = server
        .post("/api/auth/pair/start")
        .add_header("host", TUNNEL_HOST)
        .json(&json!({ "setup_token": "nope" }))
        .await;
    response.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn pair_start_caps_oversize_token() {
    let dir = TempDir::new().unwrap();
    let (server, _) = server(&dir).await;
    let huge = "a".repeat(200);
    let response = server
        .post("/api/auth/pair/start")
        .add_header("host", TUNNEL_HOST)
        .json(&json!({ "setup_token": huge }))
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);
}

// ---------- setup-tokens CRUD ----------

#[tokio::test]
async fn setup_token_create_requires_auth() {
    let dir = TempDir::new().unwrap();
    let (server, _) = server(&dir).await;
    let response = server
        .post("/api/auth/setup-tokens")
        .add_header("host", TUNNEL_HOST)
        .json(&json!({ "name": "test" }))
        .await;
    response.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn setup_token_create_on_localhost_returns_plaintext_once() {
    let dir = TempDir::new().unwrap();
    let (server, _) = server(&dir).await;
    let response = server
        .post("/api/auth/setup-tokens")
        .add_header("host", "localhost:7100")
        .json(&json!({ "name": "iPhone" }))
        .await;
    response.assert_status(StatusCode::CREATED);
    let body: serde_json::Value = response.json();
    assert!(body["plaintext"].as_str().unwrap().len() == 64);
    assert!(!body["id"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn setup_token_list_via_tunnel_with_session() {
    let dir = TempDir::new().unwrap();
    let (server, state) = server(&dir).await;
    seed_session(&state, "session-tok").await;

    let response = server
        .get("/api/auth/setup-tokens")
        .add_header("host", TUNNEL_HOST)
        .add_header("cookie", "sipag_session=session-tok")
        .await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert!(body.is_array());
}

// ---------- /login HTML branching ----------

#[tokio::test]
async fn login_renders_register_intent_on_localhost_with_no_credentials() {
    let dir = TempDir::new().unwrap();
    let (server, _) = server(&dir).await;
    let response = server
        .get("/login")
        .add_header("host", "localhost:7100")
        .await;
    response.assert_status_ok();
    assert!(response.text().contains("Register first device"));
}

#[tokio::test]
async fn login_renders_login_intent_when_credentials_exist() {
    let dir = TempDir::new().unwrap();
    let (server, state) = server(&dir).await;
    seed_session(&state, "tk").await;

    let response = server.get("/login").add_header("host", TUNNEL_HOST).await;
    response.assert_status_ok();
    assert!(response.text().contains("Sign in"));
}

#[tokio::test]
async fn login_renders_pair_intent_with_setup_token_query() {
    let dir = TempDir::new().unwrap();
    let (server, _) = server(&dir).await;
    let response = server
        .get("/login?setup_token=abc")
        .add_header("host", TUNNEL_HOST)
        .await;
    response.assert_status_ok();
    assert!(response.text().contains("Pair this device"));
}

#[tokio::test]
async fn login_renders_bootstrap_intent_when_remote_with_no_credentials_and_no_token() {
    let dir = TempDir::new().unwrap();
    let (server, _) = server(&dir).await;
    let response = server.get("/login").add_header("host", TUNNEL_HOST).await;
    response.assert_status_ok();
    assert!(response.text().contains("No passkeys yet"));
}
