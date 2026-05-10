//! Smoke test for the by-id wire shape against a live katulong.
//!
//! Run against a local katulong server (auth-bypassed on localhost):
//!
//! ```
//! cargo run --example probe_by_id
//! ```
//!
//! Reads the local server config from `~/.katulong/server.json` and
//! exercises each [`KatulongClient`] method that talks to the
//! `/sessions/by-id/...` routes. Cleans up the probe session on exit.

use anyhow::{Context, Result};
use sipag_core::katulong::KatulongClient;
use std::fs;

fn main() -> Result<()> {
    let home = std::env::var("HOME").context("HOME not set")?;
    let path = format!("{home}/.katulong/server.json");
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("read {path} (is the local katulong server running?)"))?;
    let cfg: serde_json::Value = serde_json::from_str(&raw)?;
    let port = cfg["port"].as_u64().context("server.json missing port")?;
    let host = cfg["host"].as_str().unwrap_or("127.0.0.1");
    let url = format!("http://{host}:{port}");
    println!("=> connecting to {url}");

    // Localhost bypasses auth in katulong, so any non-empty key works.
    let client = KatulongClient::new(url, "ignored-localhost".into());

    let probe_name = "probe-rust-client";

    println!("=> create_session('{probe_name}')");
    let session = client.create_session(probe_name)?;
    println!("   id={} name={}", session.id, session.name);

    println!("=> create_session again (idempotent — expect same id)");
    let session2 = client.create_session(probe_name)?;
    assert_eq!(
        session.id, session2.id,
        "idempotent create returned a different id"
    );
    println!("   ok — id matches");

    println!("=> exec_session(id, 'echo from-rust-client')");
    client.exec_session(&session.id, "echo from-rust-client")?;
    println!("   ok");

    println!("=> session_status(id)");
    let status = client.session_status(&session.id)?;
    println!(
        "   id={} name={} alive={} hasChildProcesses={}",
        status.id, status.name, status.alive, status.has_child_processes
    );
    assert_eq!(status.id, session.id);
    assert_eq!(status.name, probe_name);

    println!("=> list_sessions() — must include probe");
    let sessions = client.list_sessions()?;
    assert!(
        sessions.iter().any(|s| s.id == session.id),
        "probe session missing from list"
    );
    println!("   ok ({} session(s) total)", sessions.len());

    println!("=> kill_session(id)");
    client.kill_session(&session.id)?;
    println!("   ok");

    println!("=> all probes passed ✓");
    Ok(())
}
