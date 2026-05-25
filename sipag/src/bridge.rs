//! Shared wiring for the ollama-bridge.
//!
//! Both the lens scheduler (`serve/lens_scheduler.rs`) and the
//! dispatch gate (`dispatch_gate.rs`) call gemma through the
//! ollama-bridge daemon (`dorky-robot/ollama-bridge`, an Elixir
//! queue-and-auth-and-dedup layer in front of `ollama serve`).
//! This module is the one place that knows how to turn
//! `~/.ollama-bridge/remote.json` into the trio of consumers
//! callers actually use:
//!
//! - [`OllamaBridgeClient`] — raw wire client (enqueue / poll /
//!   probe). Lens-workers and the gate go through one of the
//!   higher-level wrappers; the raw client is here so future
//!   probe-style code paths can use it directly.
//! - [`BridgeChatBackend`] — implements `sipag_lens::ChatBackend`.
//!   `LensWorker::run_with_tools` and the dispatch gate both consume
//!   this trait, so they share the bridge for free.
//! - [`BridgeEmbedder`] — implements `sipag_corpus::Embedder`.
//!   Used by the corpus to embed lens-worker Observes so subsequent
//!   `corpus.search` calls can find them.
//!
//! All three share a single underlying `reqwest::Client` (cheap to
//! clone — Arc-internal) and a single sha256-bearer config. The
//! embedder is pinned to `nomic-embed-text` (the dorky-robot stack's
//! standard local embed model); promote to `~/.sipag/models.toml`
//! when a second embedder becomes plausible.

use anyhow::{Context, Result};
use ollama_bridge_client::{OllamaBridgeClient, RemoteConfig};
use sipag_corpus::BridgeEmbedder;
use sipag_lens::BridgeChatBackend;
use std::time::Duration;

/// Embedder model the scheduler + lens-workers use both for corpus
/// writes and for `corpus.search` queries. Local, free, stable dim.
/// Lift to `~/.sipag/models.toml` when a second embed model becomes
/// plausible.
pub const DEFAULT_EMBEDDER_MODEL: &str = "nomic-embed-text";

/// Build a reqwest client suitable for talking to the bridge from any
/// sipag surface (serve, CLI, future tools). Cloudflare's Browser
/// Integrity Check 403s the default `reqwest/x.y.z` UA when the
/// bridge sits behind a tunnel — keep "Mozilla" in the UA so we
/// always get through. 600s timeout matches gemma's worst-case
/// generation latency for the gate-tier prompt.
pub fn default_http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .user_agent(concat!(
            "Mozilla/5.0 sipag-bridge/",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .context("failed to build reqwest client for the bridge")
}

/// Constructed bridge consumers, ready to share between the
/// scheduler, the dispatch gate, and any future bridge caller.
/// Cheap to clone — `OllamaBridgeClient` is Arc-internal,
/// `BridgeChatBackend` + `BridgeEmbedder` derive `Clone` against
/// it (sipag-lens + sipag-corpus enabled this with the gate fold).
#[derive(Clone)]
pub struct BridgeWiring {
    /// Raw wire client. Hold onto it so future probe-style code
    /// (`/api/tags`, `/api/ps`) can use it directly.
    pub client: OllamaBridgeClient,
    /// `sipag_lens::ChatBackend` impl — for `LensWorker::run_with_tools`
    /// and the dispatch gate's classify call.
    pub chat: BridgeChatBackend,
    /// `sipag_corpus::Embedder` impl — for `Corpus::add_text` and
    /// the `execute_corpus_search` tool wrapper.
    pub embedder: BridgeEmbedder,
}

/// Build the full bridge wiring from `~/.ollama-bridge/remote.json`
/// plus the supplied `reqwest::Client`. The shared client carries
/// the connection pool / UA / timeout config sipag already configured
/// at startup (for serve) or constructed for the CLI one-shot.
///
/// Returns Err when the config file is missing or malformed. Callers
/// decide whether that's fatal (gate path: yes) or just disables a
/// feature (scheduler path: warn-and-continue without the loop).
pub fn build_bridge_wiring(http: reqwest::Client) -> Result<BridgeWiring> {
    let cfg = RemoteConfig::load().context(
        "load ~/.ollama-bridge/remote.json — required for the lens scheduler and the dispatch gate",
    )?;
    let client = OllamaBridgeClient::with_client(http, cfg.url, cfg.api_key);
    let chat = BridgeChatBackend::new(client.clone());
    let embedder = BridgeEmbedder::new(client.clone(), DEFAULT_EMBEDDER_MODEL.to_string());
    Ok(BridgeWiring {
        client,
        chat,
        embedder,
    })
}
