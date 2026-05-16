//! Headless katulong client CLI.
//!
//! Wraps the same `katulong_client` library that programmatic callers
//! (like sipag's dispatch handler) use, so behaviour validated from a
//! shell carries over 1:1 to library use. Designed for "drive a
//! katulong session from a terminal and watch the browser tab
//! reflect every keystroke" workflows — the validation surface for
//! the headless client.
//!
//! ```sh
//! $ katulong-client sessions
//! $ sid=$(katulong-client create | jq -r .name)
//! $ katulong-client paste "$sid" 'echo hello world'
//! $ katulong-client press "$sid" enter
//! $ katulong-client wait-for "$sid" 'hello world'
//! $ katulong-client lines "$sid" -n 20
//! ```
//!
//! Auth + base URL resolution (in order):
//!   1. CLI flags `--url <URL> --api-key <KEY>`
//!   2. Env vars `KATULONG_URL` + `KATULONG_API_KEY`
//!   3. `~/.katulong/remote.json` (`{"url":"...","apiKey":"..."}`)

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use katulong_client::{
    KatulongAttachClient, KatulongClient, KeyName, RegexMatch, RemoteConfig, WaitFrom,
};
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(
    name = "katulong-client",
    about = "Headless Rust client for katulong — drive a session from the terminal.",
    version
)]
struct Cli {
    /// Katulong base URL (overrides env + `~/.katulong/remote.json`).
    #[arg(long, env = "KATULONG_URL", global = true)]
    url: Option<String>,

    /// Katulong API key (overrides env + `~/.katulong/remote.json`).
    #[arg(long, env = "KATULONG_API_KEY", global = true)]
    api_key: Option<String>,

    /// Emit JSON instead of human-readable text where applicable.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// List sessions on the configured katulong.
    Sessions,

    /// Create a fresh dispatch session (random opaque name).
    Create {
        /// Use this name instead of a generated one.
        #[arg(long)]
        name: Option<String>,
    },

    /// Send raw bytes to a session's PTY (no bracketed paste, no
    /// trailing Enter — use `paste` + `press enter` for that).
    Input {
        /// Session name (NOT id — katulong's WS attach is keyed by name).
        session: String,
        /// Bytes to send. Use `$'\\r'` etc. from the shell for control
        /// characters.
        bytes: String,
    },

    /// Bracketed-paste a body into a session. Does NOT include a
    /// submit Enter — pair with `press <session> enter`.
    Paste { session: String, body: String },

    /// Send a named keystroke.
    Press {
        session: String,
        /// One of: enter, escape, tab, backspace, ctrl-c, ctrl-d, up, down, left, right.
        key: String,
    },

    /// Block until a regex matches the session's rolling buffer.
    WaitFor {
        session: String,
        /// Regex pattern (Rust `regex` crate syntax).
        pattern: String,
        /// `now` (default; only post-call bytes), `attach` (entire
        /// buffer including the initial snapshot), or a numeric byte
        /// offset into the stripped buffer.
        #[arg(long, default_value = "now")]
        from: String,
        /// Timeout in seconds. Default 30; use 0 for indefinite.
        #[arg(long, default_value_t = 30u64)]
        timeout: u64,
    },

    /// Print the last N lines of a session's stripped buffer.
    Lines {
        session: String,
        #[arg(short = 'n', default_value_t = 80usize)]
        n: usize,
    },

    /// Dump the raw rolling-buffer bytes (includes ANSI escapes).
    Snapshot { session: String },

    /// Print the current stripped-buffer length. Useful as a
    /// `--from <offset>` argument for a follow-up `wait-for` call.
    Offset { session: String },

    /// Serve a notebook-style web UI on `--port` that lets you click
    /// ▶ on each cell (create, paste, press, wait-for, lines, ...)
    /// and exercises the library against the configured katulong.
    /// Sessions you create appear in your real katulong's session
    /// list — the notebook is the validation surface for the actual
    /// dispatch path, not a hermetic toy.
    ///
    /// Reuses the global `--url` / `--api-key` flags (or
    /// `~/.katulong/remote.json`) to find the katulong.
    ///
    /// Gated behind the `serve` Cargo feature (default-on for the
    /// binary; library-only consumers can disable it with
    /// `default-features = false` to skip axum's compile cost).
    #[cfg(feature = "serve")]
    Serve {
        /// Port the notebook UI listens on. Open `http://127.0.0.1:<port>`.
        #[arg(long, default_value_t = 8765u16)]
        port: u16,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Tracing only when the user opts in via RUST_LOG — keeps `--json`
    // output uncontaminated by log lines on stderr by default.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

    let cli = Cli::parse();
    let remote = resolve_remote(&cli)?;

    match cli.cmd {
        Cmd::Sessions => run_sessions(&remote, cli.json),
        Cmd::Create { name } => run_create(&remote, name.as_deref(), cli.json),
        Cmd::Input { session, bytes } => run_input(&remote, &session, &bytes).await,
        Cmd::Paste { session, body } => run_paste(&remote, &session, &body).await,
        Cmd::Press { session, key } => run_press(&remote, &session, &key).await,
        Cmd::WaitFor {
            session,
            pattern,
            from,
            timeout,
        } => run_wait_for(&remote, &session, &pattern, &from, timeout, cli.json).await,
        Cmd::Lines { session, n } => run_lines(&remote, &session, n).await,
        Cmd::Snapshot { session } => run_snapshot(&remote, &session).await,
        Cmd::Offset { session } => run_offset(&remote, &session).await,
        #[cfg(feature = "serve")]
        Cmd::Serve { port } => {
            katulong_client::serve::run(katulong_client::serve::ServeOpts {
                port,
                remote: remote.clone(),
            })
            .await
        }
    }
}

/// Resolve the katulong endpoint + api key. CLI flag wins, then env
/// (handled by `clap`'s `env =` already), then `~/.katulong/remote.json`.
fn resolve_remote(cli: &Cli) -> Result<RemoteConfig> {
    if let (Some(url), Some(api_key)) = (&cli.url, &cli.api_key) {
        return Ok(RemoteConfig {
            url: url.clone(),
            api_key: api_key.clone(),
        });
    }
    // Fall through to the file. If only one of url/api_key is set on
    // the CLI, the file fills the other.
    let from_file = RemoteConfig::load().ok();
    let url = cli
        .url
        .clone()
        .or_else(|| from_file.as_ref().map(|c| c.url.clone()))
        .ok_or_else(|| {
            anyhow!(
                "no katulong URL: pass --url, set KATULONG_URL, or write ~/.katulong/remote.json"
            )
        })?;
    let api_key = cli
        .api_key
        .clone()
        .or_else(|| from_file.as_ref().map(|c| c.api_key.clone()))
        .ok_or_else(|| {
            anyhow!(
                "no katulong api key: pass --api-key, set KATULONG_API_KEY, or write \
                 ~/.katulong/remote.json"
            )
        })?;
    Ok(RemoteConfig { url, api_key })
}

fn http_client(remote: &RemoteConfig) -> KatulongClient {
    KatulongClient::new(remote.url.clone(), remote.api_key.clone())
}

fn ws_client(remote: &RemoteConfig) -> KatulongAttachClient {
    KatulongAttachClient::new(remote.clone())
}

// ── synchronous (HTTP-only) commands ────────────────────────────

fn run_sessions(remote: &RemoteConfig, as_json: bool) -> Result<()> {
    let sessions = http_client(remote)
        .list_sessions()
        .context("list sessions")?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&sessions)?);
    } else {
        for s in &sessions {
            println!("{}\t{}", s.id, s.name);
        }
    }
    Ok(())
}

fn run_create(remote: &RemoteConfig, name: Option<&str>, as_json: bool) -> Result<()> {
    let http = http_client(remote);
    let session = match name {
        Some(n) => http.create_session(n)?,
        None => http.create_dispatch_session()?,
    };
    if as_json {
        println!("{}", serde_json::to_string_pretty(&session)?);
    } else {
        println!("{}\t{}", session.id, session.name);
    }
    Ok(())
}

// ── async (WS attach) commands ──────────────────────────────────

const DEFAULT_COLS: u16 = katulong_client::attach::DEFAULT_ATTACH_COLS;
const DEFAULT_ROWS: u16 = katulong_client::attach::DEFAULT_ATTACH_ROWS;

async fn attach(remote: &RemoteConfig, session: &str) -> Result<katulong_client::KatulongAttach> {
    ws_client(remote)
        .attach(session, DEFAULT_COLS, DEFAULT_ROWS)
        .await
        .with_context(|| format!("attach to session '{session}'"))
}

async fn run_input(remote: &RemoteConfig, session: &str, bytes: &str) -> Result<()> {
    let attach = attach(remote, session).await?;
    attach.input(bytes).await.context("input")?;
    attach.close().await;
    Ok(())
}

async fn run_paste(remote: &RemoteConfig, session: &str, body: &str) -> Result<()> {
    // `paste` is now a thin wrapper over `input` — same wire shape
    // xterm.js uses on a paste event. The subcommand stays named
    // `paste` because operators think of it that way; under the
    // hood it's just `input(body)`.
    let attach = attach(remote, session).await?;
    attach
        .input(body.to_string())
        .await
        .context("paste/input")?;
    attach.close().await;
    Ok(())
}

async fn run_press(remote: &RemoteConfig, session: &str, key: &str) -> Result<()> {
    let key = parse_key(key)?;
    let attach = attach(remote, session).await?;
    attach.press(key).await.context("press")?;
    attach.close().await;
    Ok(())
}

async fn run_wait_for(
    remote: &RemoteConfig,
    session: &str,
    pattern: &str,
    from: &str,
    timeout_secs: u64,
    as_json: bool,
) -> Result<()> {
    let re = regex::Regex::new(pattern).with_context(|| format!("compile pattern: {pattern}"))?;
    let attach = attach(remote, session).await?;
    let from = parse_wait_from(from, &attach).await?;
    let timeout = if timeout_secs == 0 {
        None
    } else {
        Some(Duration::from_secs(timeout_secs))
    };
    let m = attach.wait_for(&re, from, timeout).await?;
    print_match(&m, as_json)?;
    attach.close().await;
    Ok(())
}

async fn run_lines(remote: &RemoteConfig, session: &str, n: usize) -> Result<()> {
    let attach = attach(remote, session).await?;
    let lines = attach.last_n_lines(n).await;
    attach.close().await;
    for line in lines {
        println!("{line}");
    }
    Ok(())
}

async fn run_snapshot(remote: &RemoteConfig, session: &str) -> Result<()> {
    use std::io::Write as _;
    let attach = attach(remote, session).await?;
    let buf = attach.buffer_snapshot().await;
    attach.close().await;
    // Raw bytes — write to stdout without UTF-8 conversion so escape
    // sequences survive.
    std::io::stdout().write_all(&buf)?;
    Ok(())
}

async fn run_offset(remote: &RemoteConfig, session: &str) -> Result<()> {
    let attach = attach(remote, session).await?;
    let offset = attach.stripped_offset().await;
    attach.close().await;
    println!("{offset}");
    Ok(())
}

// ── parsers ─────────────────────────────────────────────────────

fn parse_key(s: &str) -> Result<KeyName> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "enter" | "return" | "\\r" => KeyName::Enter,
        "escape" | "esc" => KeyName::Escape,
        "tab" => KeyName::Tab,
        "backspace" | "bs" => KeyName::Backspace,
        "ctrl-c" | "ctrlc" | "^c" => KeyName::CtrlC,
        "ctrl-d" | "ctrld" | "^d" => KeyName::CtrlD,
        "up" => KeyName::Up,
        "down" => KeyName::Down,
        "left" => KeyName::Left,
        "right" => KeyName::Right,
        other => {
            return Err(anyhow!(
                "unknown key '{other}' — try: enter, escape, tab, backspace, ctrl-c, ctrl-d, up, down, left, right"
            ));
        }
    })
}

async fn parse_wait_from(s: &str, attach: &katulong_client::KatulongAttach) -> Result<WaitFrom> {
    Ok(match s {
        "now" => WaitFrom::FromNow,
        "attach" | "from-attach" => WaitFrom::FromAttach,
        other => {
            let offset: usize = other.parse().with_context(|| {
                format!("--from must be `now`, `attach`, or a byte offset (got `{other}`)")
            })?;
            // If the caller said "0" they probably meant attach; if
            // they said the current offset they likely captured it
            // via `offset` earlier. Just pass through verbatim.
            let _ = attach; // borrow for symmetry with future overrides
            WaitFrom::FromOffset(offset)
        }
    })
}

fn print_match(m: &RegexMatch, as_json: bool) -> Result<()> {
    if as_json {
        let v = serde_json::json!({
            "start": m.start,
            "end": m.end,
            "matched_text": m.matched_text,
        });
        println!("{}", serde_json::to_string(&v)?);
    } else {
        // Just print the matched text — composable in pipelines.
        println!("{}", m.matched_text);
    }
    Ok(())
}
