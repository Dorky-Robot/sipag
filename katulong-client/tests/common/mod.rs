//! Shared test harness — spawns a real `katulong` server as a
//! subprocess on a free loopback port with an isolated state dir,
//! kills it on drop. Designed so each integration test file owns a
//! fresh katulong instance and tests can't bleed state into each
//! other.
//!
//! Activated via `KATULONG_REPO=/path/to/katulong-checkout`. Tests
//! that depend on the harness should early-return when the var is
//! missing so CI without a katulong checkout stays green.
//!
//! Note: each `tests/*.rs` is its own binary, so unused-helper
//! warnings fire per-binary if a given test file doesn't reach for
//! every helper. `#[allow(dead_code)]` keeps the shared module
//! warning-clean regardless of which subset is in use.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// One running katulong server. Owns the child process; killed on
/// drop so a panicking test doesn't strand the subprocess or its
/// tmux PTYs.
pub struct KatulongHarness {
    child: Option<Child>,
    pub port: u16,
    /// Temp dir used as `KATULONG_DATA_DIR`. Kept alive until the
    /// harness drops so katulong can finish writing state.
    _data_dir: tempfile::TempDir,
    /// Per-harness tmux socket name. The harness kills its tmux
    /// server on drop so the socket and any orphaned PTYs go with it.
    tmux_socket: String,
}

impl KatulongHarness {
    /// Spawn katulong on a free port using a fresh state dir. Returns
    /// `Ok(None)` when `KATULONG_REPO` isn't set — tests should
    /// skip cleanly.
    pub fn try_start() -> std::io::Result<Option<Self>> {
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
        // Per-harness tmux socket — see serve.rs for the full
        // rationale. Without this the harness shares the default
        // tmux socket with any other katulong on the operator's
        // box, which adopts the session and detaches our control
        // client mid-test. The free port doubles as a guaranteed-
        // unique suffix so parallel `cargo test` workers (or two
        // harnesses inside one test) never collide on the socket
        // name.
        let tmux_socket = format!("sipag-test-{}-{port}", std::process::id());

        let child = Command::new("node")
            .arg(&server_js)
            .current_dir(&repo)
            .env("PORT", port.to_string())
            .env("KATULONG_BIND_HOST", "127.0.0.1")
            .env("KATULONG_DATA_DIR", data_dir.path())
            .env("KATULONG_TMUX_SOCKET", &tmux_socket)
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
            tmux_socket,
        };

        // Poll the HTTP listener until ready. Katulong's Node boot
        // takes ~300ms-2s on this hardware; cap at 15s.
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if let Ok(status) = http_head_status(&format!("http://127.0.0.1:{port}/sessions")) {
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

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

impl Drop for KatulongHarness {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // Kill the per-harness tmux server (otherwise it outlives the
        // node child and orphan PTYs accumulate over CI runs).
        let _ = Command::new("tmux")
            .args(["-L", &self.tmux_socket, "kill-server"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn free_port() -> std::io::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

/// Minimal HTTP GET that reads just the start of the response.
/// Used for readiness polling; avoids pulling in a runtime HTTP dep
/// for a probe loop.
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

/// Convenience: the SKIP message printed by tests that depend on the
/// harness when `KATULONG_REPO` is unset. Keep wording consistent so
/// CI grep filters can flag intentional skips vs. real failures.
pub const SKIP_MSG: &str =
    "KATULONG_REPO not set or invalid; skipping katulong-client integration test";
