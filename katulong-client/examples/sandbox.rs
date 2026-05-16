//! Interactive sandbox: spawn a fresh katulong subprocess on a free
//! local port, create a dispatch session, print the URL + env vars
//! you need to drive it, then sit alive until Ctrl-C. Open the URL
//! in your browser to watch the session in real time; drive it from
//! another shell with the `katulong-client` CLI.
//!
//! ```sh
//! # terminal 1:
//! $ KATULONG_REPO=/path/to/katulong-checkout \
//!     cargo run -p katulong-client --example sandbox
//!
//! # terminal 2 (in another tab; copy the env block from terminal 1):
//! $ export KATULONG_URL=http://127.0.0.1:NNNN
//! $ export KATULONG_API_KEY=unused-because-localhost
//! $ katulong-client paste $SESSION_NAME 'echo hello'
//! $ katulong-client press $SESSION_NAME enter
//! $ katulong-client wait-for $SESSION_NAME 'hello' --from attach --timeout 5
//! $ katulong-client lines $SESSION_NAME -n 20
//! ```
//!
//! The browser tab at http://127.0.0.1:NNNN/ will reflect every
//! keystroke the CLI sends — that's the validation loop. Ctrl-C
//! in terminal 1 to tear down (kills katulong subprocess + tmux
//! sessions it spawned).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use katulong_client::{KatulongClient, RemoteConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let repo = std::env::var("KATULONG_REPO").map_err(|_| {
        anyhow::anyhow!(
            "set KATULONG_REPO=/path/to/katulong-checkout — the sandbox spawns it as a subprocess"
        )
    })?;
    let repo = PathBuf::from(repo);
    let server_js = repo.join("server.js");
    if !server_js.exists() {
        anyhow::bail!("KATULONG_REPO does not contain a server.js: {repo:?}");
    }

    let port = free_port()?;
    let data_dir = tempfile::tempdir()?;
    println!(
        "[sandbox] spawning katulong on 127.0.0.1:{port} (state in {:?})",
        data_dir.path()
    );

    // Per-sandbox tmux socket — see the comment in serve.rs for the
    // full rationale. Short version: without this the sandbox shares
    // the default tmux socket with the operator's other katulong
    // instance, which adopts our session and detaches our control
    // client, killing the attach with exit code 0.
    let tmux_socket = format!("sipag-sandbox-{}", std::process::id());

    let mut child = Command::new("node")
        .arg(&server_js)
        .current_dir(&repo)
        .env("PORT", port.to_string())
        .env("KATULONG_BIND_HOST", "127.0.0.1")
        .env("KATULONG_DATA_DIR", data_dir.path())
        .env("KATULONG_TMUX_SOCKET", &tmux_socket)
        .env("LOG_LEVEL", "info")
        .env("NODE_ENV", "production")
        // Show katulong's stdout/stderr so operator sees what's
        // happening — it's a sandbox, after all.
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;

    // RAII teardown — kill katulong if anything below panics or
    // when main returns naturally on Ctrl-C.
    let _guard = TeardownGuard {
        child: &mut child,
        data_dir,
        tmux_socket: tmux_socket.clone(),
    };

    wait_until_ready(port, Duration::from_secs(15))?;
    println!("[sandbox] katulong is up");

    let remote = RemoteConfig {
        url: format!("http://127.0.0.1:{port}"),
        api_key: "unused-because-localhost".to_string(),
    };
    let http = KatulongClient::new(remote.url.clone(), remote.api_key.clone());
    let session = http.create_dispatch_session()?;
    println!(
        "[sandbox] created session: name={} id={}",
        session.name, session.id
    );

    println!();
    println!("─────────────────────────────────────────────────────────────────────────");
    println!(" Open this URL in your browser to watch the session in real time:");
    println!("   {}/", remote.url);
    println!();
    println!(" Drive it from another shell:");
    println!("   export KATULONG_URL={}", remote.url);
    println!("   export KATULONG_API_KEY={}", remote.api_key);
    println!("   export SID={}", session.name);
    println!();
    println!("   katulong-client paste \"$SID\" 'echo hello world'");
    println!("   katulong-client press \"$SID\" enter");
    println!("   katulong-client wait-for \"$SID\" 'hello world' --from attach --timeout 5");
    println!("   katulong-client lines \"$SID\" -n 20");
    println!();
    println!(" Press Ctrl-C in THIS terminal to tear down.");
    println!("─────────────────────────────────────────────────────────────────────────");

    // Sit and wait for Ctrl-C. The `_guard` drop above kills the
    // katulong subprocess + cleans the state dir as part of
    // function-return stack unwinding.
    tokio::signal::ctrl_c().await?;
    println!();
    println!("[sandbox] tearing down");
    Ok(())
}

// ── helpers ─────────────────────────────────────────────────────

struct TeardownGuard<'a> {
    child: &'a mut Child,
    // Hold the tempdir alive until drop runs, then it's deleted.
    data_dir: tempfile::TempDir,
    tmux_socket: String,
}

impl Drop for TeardownGuard<'_> {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // Kill the per-sandbox tmux server (otherwise it outlives the
        // node child and leaks on the operator's box).
        let _ = std::process::Command::new("tmux")
            .args(["-L", &self.tmux_socket, "kill-server"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let path = self.data_dir.path().to_path_buf();
        // tempdir's drop deletes the directory; we just announce.
        println!("[sandbox] katulong killed, state dir cleaned: {path:?}");
    }
}

fn free_port() -> std::io::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

fn wait_until_ready(port: u16, max: Duration) -> std::io::Result<()> {
    let deadline = Instant::now() + max;
    while Instant::now() < deadline {
        if let Ok(status) = http_head_status(&format!("http://127.0.0.1:{port}/sessions")) {
            if (200..500).contains(&status) {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(std::io::Error::other(format!(
        "katulong did not respond on 127.0.0.1:{port}/sessions within {max:?}"
    )))
}

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
