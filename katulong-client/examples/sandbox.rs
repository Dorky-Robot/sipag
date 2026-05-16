//! Interactive sandbox: connect to a real katulong (the one configured
//! at `~/.katulong/remote.json`, the same one production sipag dispatches
//! against), create a fresh `sipag-d-…` dispatch session, print the env
//! vars needed to drive it, then sit alive until Ctrl-C. The session is
//! deleted on teardown so it doesn't pollute your daily-driver's session
//! list.
//!
//! Set `KATULONG_URL` / `KATULONG_API_KEY` or write `~/.katulong/remote.json`
//! before running. On localhost katulong, the api key isn't enforced —
//! `isLocalRequest` in the server bypasses auth — so any string works.
//!
//! ```sh
//! # terminal 1:
//! $ cargo run -p katulong-client --example sandbox
//!
//! # terminal 2 (copy the env block from terminal 1):
//! $ export KATULONG_URL=…
//! $ export KATULONG_API_KEY=…
//! $ export SID=sipag-d-…
//! $ katulong-client paste "$SID" 'echo hello world'
//! $ katulong-client press "$SID" enter
//! $ katulong-client wait-for "$SID" 'hello world' --from attach --timeout 5
//! $ katulong-client lines "$SID" -n 20
//! ```
//!
//! Open the katulong URL in your browser to watch the session in real
//! time. Ctrl-C in terminal 1 to tear down (deletes the sandbox session
//! from your katulong; leaves everything else alone).

use anyhow::{anyhow, Context, Result};
use katulong_client::{KatulongClient, RemoteConfig};

#[tokio::main]
async fn main() -> Result<()> {
    let remote = RemoteConfig::load().map_err(|e| {
        anyhow!(
            "no katulong configured ({e}). Set KATULONG_URL + KATULONG_API_KEY or write ~/.katulong/remote.json"
        )
    })?;

    let http = KatulongClient::new(remote.url.clone(), remote.api_key.clone());
    let session = http
        .create_dispatch_session()
        .with_context(|| format!("create session on {}", remote.url))?;
    println!(
        "[sandbox] created session on {}: name={} id={}",
        remote.url, session.name, session.id
    );

    println!();
    println!("─────────────────────────────────────────────────────────────────────────");
    println!(" Open this URL in your browser to watch the session in real time:");
    println!("   {}/?s={}", remote.url, urlencoding_encode(&session.name));
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
    println!(" Press Ctrl-C in THIS terminal to delete the sandbox session.");
    println!("─────────────────────────────────────────────────────────────────────────");

    tokio::signal::ctrl_c().await?;
    println!();
    println!("[sandbox] cleaning up session {}", session.name);
    let _ = http.kill_session(&session.id);
    Ok(())
}

/// Minimal URL-encoder for the one query-param value we emit. Avoids
/// pulling `urlencoding` into the example just for this. Covers only
/// the characters that actually appear in a `sipag-d-<hex>` session
/// name — alphanumeric + `-` — by passing them through; anything else
/// is hex-escaped.
fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
