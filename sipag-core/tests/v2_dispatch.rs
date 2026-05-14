//! End-to-end tests for the v2 dispatch attach client against a
//! real `katulong` server. Designed to catch protocol mismatches at
//! the layer where they actually matter — wire format, WS upgrade
//! requirements (Origin), session-name lookup keys, drift signals
//! parsed as the bytes katulong actually emits.
//!
//! Activated via `KATULONG_REPO=/path/to/katulong-checkout`. Tests
//! print a skip notice and pass when the env var is missing.

use sipag_core::katulong::client::{KatulongAttachClient, WaitFrom};
use sipag_core::katulong::{KatulongClient, RemoteConfig};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Bootstrap test: drive `KatulongAttachClient` against a real
/// katulong subprocess. Validates the full WS-level handshake, the
/// session-name lookup path (not session-id), the inbound-message
/// parsing (state-check with integer fingerprint), and the
/// keystroke round-trip (input → PTY → output → rolling buffer →
/// `wait_for` match).
///
/// Uses a shell `printf` rather than a `claude` launch so the test
/// is shell-only and doesn't depend on a claude binary on PATH.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attach_input_round_trip_against_real_katulong() {
    let Some(harness) = KatulongHarness::try_start().expect("spawn katulong") else {
        eprintln!("KATULONG_REPO not set or invalid; skipping v2 e2e tests");
        return;
    };

    // Localhost auth bypass — katulong treats 127.0.0.1 callers as
    // authenticated, so we don't need to mint an API key for the
    // happy path. A non-local Origin test will need its own setup.
    let api_key = "unused-because-localhost".to_string();
    let http = KatulongClient::new(harness.url(), api_key.clone());
    let session = http.create_dispatch_session().expect("create session");

    let attach_client = KatulongAttachClient::new(RemoteConfig {
        url: harness.url(),
        api_key,
    });

    let attach = attach_client
        .attach(&session.name, 120, 40)
        .await
        .expect("WS attach");

    // Unique sentinel so a shell prompt or banner can't satisfy
    // the wait by accident.
    let token = "v2-dispatch-roundtrip-sentinel";
    let cmd = format!("printf '%s\\n' '{token}'\r");
    let offset_before = attach.stripped_offset().await;
    attach.input(&cmd).await.expect("input(echo)");

    let pat = regex::Regex::new(token).expect("compile");
    let m = attach
        .wait_for(
            &pat,
            WaitFrom::FromOffset(offset_before),
            Some(Duration::from_secs(10)),
        )
        .await
        .expect("wait_for token after input round-trip");

    assert_eq!(m.matched_text, token);

    // Close MUST terminate quickly — wrap in a timeout so a
    // regression to the buggy reader/writer/close ordering (where
    // the reader's `writer_tx.clone()` keeps the channel open and
    // the writer task hangs forever on `rx.recv()`) fails the test
    // loudly instead of hanging it.
    tokio::time::timeout(Duration::from_secs(2), attach.close())
        .await
        .expect("close hung — likely a writer-task deadlock regression");
}

// ── harness ─────────────────────────────────────────────────────────

/// One running katulong server. Owns the child process; killed on
/// drop so a panicking test doesn't strand the subprocess or its
/// tmux PTYs.
struct KatulongHarness {
    child: Option<Child>,
    port: u16,
    /// Temp dir used as `KATULONG_DATA_DIR`. Kept alive until the
    /// harness drops so katulong can finish writing state.
    _data_dir: tempfile::TempDir,
}

impl KatulongHarness {
    /// Spawn katulong on a free port using a fresh state dir. Returns
    /// `Ok(None)` when `KATULONG_REPO` isn't set — the test should
    /// skip cleanly.
    fn try_start() -> std::io::Result<Option<Self>> {
        let Ok(repo) = std::env::var("KATULONG_REPO") else {
            return Ok(None);
        };
        let repo = PathBuf::from(repo);
        let server_js = repo.join("server.js");
        if !server_js.exists() {
            return Ok(None);
        }

        let port = free_port()?;
        let data_dir = tempfile::tempdir()?;

        let child = Command::new("node")
            .arg(&server_js)
            .current_dir(&repo)
            .env("PORT", port.to_string())
            .env("KATULONG_BIND_HOST", "127.0.0.1")
            .env("KATULONG_DATA_DIR", data_dir.path())
            .env("LOG_LEVEL", "warn")
            .env("NODE_ENV", "production")
            // PATH/SHELL/HOME inherited — katulong needs tmux + shell.
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;

        let harness = KatulongHarness {
            child: Some(child),
            port,
            _data_dir: data_dir,
        };

        // Poll the HTTP listener until ready. Katulong's Node boot
        // takes ~300ms-2s on this hardware; cap at 15s.
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if let Ok(status) = http_head_status(&format!("http://127.0.0.1:{port}/sessions")) {
                // 2xx happy path; 401/403 still means listener is up.
                if (200..500).contains(&status) {
                    return Ok(Some(harness));
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(std::io::Error::other(
            "katulong did not respond on /sessions within 15s",
        ))
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

impl Drop for KatulongHarness {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn free_port() -> std::io::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

/// Minimal HTTP GET that reads just the status code from the start
/// of the response. Used for readiness polling; avoids pulling in a
/// runtime HTTP dep for a probe loop.
fn http_head_status(url: &str) -> std::io::Result<u16> {
    let parsed = url::Url::parse(url).map_err(std::io::Error::other)?;
    let host = parsed
        .host_str()
        .ok_or_else(|| std::io::Error::other("missing host"))?;
    let port = parsed.port().unwrap_or(80);
    let path = parsed.path();

    let mut sock = TcpStream::connect((host, port))?;
    sock.set_read_timeout(Some(Duration::from_millis(500)))?;
    sock.set_write_timeout(Some(Duration::from_millis(500)))?;
    let req = format!("GET {path} HTTP/1.0\r\nHost: {host}\r\n\r\n");
    sock.write_all(req.as_bytes())?;
    let mut buf = [0u8; 64];
    let n = sock.read(&mut buf)?;
    let head = std::str::from_utf8(&buf[..n]).unwrap_or("");
    head.split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| std::io::Error::other(format!("bad status line: {head:?}")))
}
