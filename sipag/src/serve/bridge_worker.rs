//! Bridge lens-worker — first reactive worker in sipag.
//!
//! Subscribes to katulong `claude/<uuid>` SSE topics and fires
//! gemma against a sliding window of session events to produce
//! observations and structural verbs. This closes the remainder
//! of Phase 1 #3 + Phase 2 #7 in `docs/architecture.md` §9.
//!
//! ## Architecture
//!
//! The bridge worker manages N per-session subscriber tasks:
//!
//! ```text
//! dispatch handler ─── topic ───► BridgeHandle::watch(topic)
//!                                       │
//!                                       ▼
//!                                 per-session task
//!                               ┌───────────────┐
//!                               │ SSE subscribe  │◄─── reconnect loop
//!                               │ SessionWindow  │     (from_seq resume)
//!                               │ trigger eval   │
//!                               │ gemma fire     │───► corpus write
//!                               └───────────────┘
//! ```
//!
//! Each session gets its own [`SessionWindow`] (bounded
//! `VecDeque<KatulongEvent>`, max [`WINDOW_CAP`] = 200 events)
//! and its own trigger state. The trigger fires gemma when either:
//!
//! - **Threshold** (v1): N events have accumulated since the last
//!   fire (default [`FIRE_THRESHOLD`] = 10).
//! - **Blind-spot** (v1: deferred): 20+ events in 5 min + no
//!   action produced. Deferred until per-fire outcome tracking
//!   exists — see `should_fire` doc comment for rationale.
//!
//! ## Prompt shape
//!
//! The bridge carries a built-in system prompt (not a lens TOML
//! file). Each fire builds a user prompt from the sliding window
//! via [`render_prompt`] (pure function, unit-testable without
//! gemma) and invokes `LensWorker::run_with_tools` for the
//! multi-turn tool-calling loop (corpus.search / corpus.expand).
//!
//! ## Structural verb dispatch (v1)
//!
//! `Observe` actions are written to the corpus (same as the
//! scheduler). `SuggestStance` / `AskHuman` / `ProposeTask` are
//! logged at `warn!` — the UI dispatch surface is a follow-up.
//!
//! ## Session discovery
//!
//! Sessions are fed via a channel: the dispatch handler sends
//! `claude/<uuid>` topic strings after successful dispatch. A
//! startup scan of active katulong sessions is deferred until the
//! `TmuxSession` wire type carries Claude metadata (or a
//! session-metadata HTTP endpoint is added to katulong).

use futures::StreamExt;
use katulong_client::sse::{subscribe, KatulongEvent, SseError};
use katulong_client::RemoteConfig;
use sipag_corpus::{Corpus, Embedder};
use sipag_lens::{
    ChatBackend, Lens, LensSource, LensWorker, ModelChoice, ModelResolver, StructuralAction,
    TriggerPolicy,
};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, info, warn};

/// Max events retained per session.
pub const WINDOW_CAP: usize = 200;

/// Fire gemma after this many new events since the last fire.
pub const FIRE_THRESHOLD: usize = 10;

/// Reconnect delay after an SSE stream ends or errors.
const RECONNECT_DELAY: Duration = Duration::from_secs(5);

/// Max tool-call iterations per gemma invocation (matches the
/// scheduler's default).
const MAX_TOOL_ITERATIONS: usize = 5;

/// Handle returned by [`spawn`] so the dispatch handler can feed
/// new session topics to the bridge worker at runtime.
#[derive(Clone)]
pub struct BridgeHandle {
    tx: mpsc::UnboundedSender<String>,
}

impl BridgeHandle {
    /// Tell the bridge worker to start watching a new session topic.
    /// Idempotent — sending the same topic twice is a no-op (the
    /// worker deduplicates internally).
    pub fn watch(&self, topic: String) {
        let _ = self.tx.send(topic);
    }
}

/// Per-session sliding window + trigger state.
pub struct SessionWindow {
    events: VecDeque<KatulongEvent>,
    events_since_fire: usize,
    last_fire: Option<Instant>,
    /// Highest seq seen — used for SSE reconnect resumption.
    last_seq: u64,
}

impl SessionWindow {
    fn new() -> Self {
        Self {
            events: VecDeque::with_capacity(WINDOW_CAP),
            events_since_fire: 0,
            last_fire: None,
            last_seq: 0,
        }
    }

    fn push(&mut self, event: KatulongEvent) {
        self.last_seq = self.last_seq.max(event.seq);
        self.events_since_fire += 1;
        self.events.push_back(event);
        if self.events.len() > WINDOW_CAP {
            self.events.pop_front();
        }
    }

    fn resume_seq(&self) -> u64 {
        if self.last_seq == 0 {
            0
        } else {
            self.last_seq + 1
        }
    }
}

/// Evaluate whether the trigger should fire.
///
/// v1: pure event-count threshold. The §3 "blind-spot" trigger
/// (20 events in 5min + no action → escalated fire with a
/// different prompt strategy) is deferred until we have production
/// telemetry on action-production rates — it can't be cleanly
/// separated from the threshold (which fires first at 10 events)
/// without a per-fire outcome signal that v1 doesn't track yet.
pub fn should_fire(window: &SessionWindow) -> bool {
    window.events_since_fire >= FIRE_THRESHOLD
}

/// Build the user prompt from the sliding window. Pure function —
/// unit-testable without a live gemma or network.
pub fn render_prompt(window: &[KatulongEvent]) -> String {
    use std::fmt::Write;
    let mut prompt = String::with_capacity(4096);
    let _ = writeln!(
        prompt,
        "Here are the {} most recent events from the session:\n",
        window.len()
    );
    for evt in window {
        let session_tag = evt
            .session
            .as_deref()
            .map(|s| format!(" session={s}"))
            .unwrap_or_default();
        let _ = writeln!(
            prompt,
            "[seq={} t={}{session_tag}] {}: {}",
            evt.seq,
            evt.timestamp,
            evt.event,
            summarize_extra(&evt.extra),
        );
    }
    let _ = writeln!(
        prompt,
        "\nAnalyze these events. Use corpus.search to find relevant \
         Key Results and prior observations. Then produce your actions."
    );
    prompt
}

fn summarize_extra(extra: &serde_json::Map<String, serde_json::Value>) -> String {
    if extra.is_empty() {
        return String::new();
    }
    // Compact JSON, truncated to keep prompts bounded.
    let raw = serde_json::to_string(extra).unwrap_or_default();
    if raw.len() <= 500 {
        raw
    } else {
        let mut s = raw;
        // Safe truncation: extra is serialized from a Map (always
        // valid UTF-8) so byte == char boundary for ASCII JSON keys.
        // For non-ASCII values, walk back to boundary.
        let target = 500;
        let mut cap = target;
        while !s.is_char_boundary(cap) {
            cap -= 1;
        }
        s.truncate(cap);
        s.push('…');
        s
    }
}

/// The bridge worker's built-in system prompt. Tells gemma what
/// it's looking at and what structural verbs are available.
const BRIDGE_SYSTEM_PROMPT: &str = "\
You are a bridge observer watching a Claude coding session in real time. \
Your job is to notice what's happening, surface insights to the human \
steering the fleet, and connect session activity to the broader OKR context.

You will receive a window of recent session events (permission requests, \
tool uses, agent completions, etc.). Analyze them and produce actions.

Available actions (respond as JSON with an \"actions\" array):

1. observe — Record a free-form observation about what's happening.
   {\"type\": \"observe\", \"content\": \"...\", \"tags\": [\"session=<id>\", ...]}

2. suggest_stance — Propose a KR stance change based on what you see.
   {\"type\": \"suggest_stance\", \"kr_ref\": \"<kr-id>\", \"stance\": \"green|yellow|red\", \"reason\": \"...\"}

3. ask_human — Surface a question the human should weigh in on.
   {\"type\": \"ask_human\", \"question\": \"...\", \"kind\": \"permission|progress-check|strategic\"}

4. propose_task — Suggest a new task based on what you observe.
   {\"type\": \"propose_task\", \"title\": \"...\", \"body\": \"...\"}

5. no_action_warranted — Explicitly signal that nothing needs attention.
   {\"type\": \"no_action_warranted\"}

Use corpus.search to find relevant Key Results, prior observations, and \
context before deciding. Not every batch of events needs an observation — \
quiet productive sessions are fine. Only surface what matters.

Respond with: {\"actions\": [...]}";

fn bridge_lens() -> Lens {
    Lens {
        name: "bridge".to_string(),
        prompt_text: BRIDGE_SYSTEM_PROMPT.to_string(),
        source: LensSource::ProjectMeta {
            name: "bridge-observer".to_string(),
        },
        model: ModelChoice::Profile(sipag_lens::Profile::Fast),
        trigger: TriggerPolicy::Threshold,
        retired: false,
    }
}

/// Spawn the bridge worker on the tokio runtime. Returns a
/// [`BridgeHandle`] the dispatch handler uses to feed new session
/// topics.
pub fn spawn<B>(
    backend: B,
    embedder: sipag_corpus::BridgeEmbedder,
    corpus: Arc<Mutex<Corpus>>,
    resolver: ModelResolver,
    remote: RemoteConfig,
    http: reqwest::Client,
) -> BridgeHandle
where
    B: ChatBackend + Clone + 'static,
{
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(run_coordinator(
        rx, backend, embedder, corpus, resolver, remote, http,
    ));
    BridgeHandle { tx }
}

async fn run_coordinator<B>(
    mut rx: mpsc::UnboundedReceiver<String>,
    backend: B,
    embedder: sipag_corpus::BridgeEmbedder,
    corpus: Arc<Mutex<Corpus>>,
    resolver: ModelResolver,
    remote: RemoteConfig,
    http: reqwest::Client,
) where
    B: ChatBackend + Clone + 'static,
{
    let mut watched: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();
    info!("bridge worker: coordinator started — waiting for session topics");

    while let Some(topic) = rx.recv().await {
        if watched.contains_key(&topic) {
            debug!(topic = %topic, "bridge worker: topic already watched, skipping");
            continue;
        }
        info!(topic = %topic, "bridge worker: spawning session subscriber");
        let handle = tokio::spawn(run_session(
            topic.clone(),
            backend.clone(),
            embedder.clone(),
            corpus.clone(),
            resolver.clone(),
            remote.clone(),
            http.clone(),
        ));
        watched.insert(topic, handle);
    }
    info!("bridge worker: coordinator channel closed — shutting down");
}

async fn run_session<B>(
    topic: String,
    backend: B,
    embedder: sipag_corpus::BridgeEmbedder,
    corpus: Arc<Mutex<Corpus>>,
    resolver: ModelResolver,
    remote: RemoteConfig,
    http: reqwest::Client,
) where
    B: ChatBackend + 'static,
{
    let lens = bridge_lens();
    let mut window = SessionWindow::new();

    loop {
        let from_seq = window.resume_seq();
        info!(
            topic = %topic,
            from_seq,
            "bridge worker: subscribing"
        );

        let stream =
            match subscribe(http.clone(), &remote.url, &remote.api_key, &topic, from_seq).await {
                Ok(s) => s,
                Err(e) => {
                    warn!(
                        topic = %topic,
                        error = %e,
                        "bridge worker: connect failed — retrying in {}s",
                        RECONNECT_DELAY.as_secs()
                    );
                    tokio::time::sleep(RECONNECT_DELAY).await;
                    continue;
                }
            };

        let mut stream = std::pin::pin!(stream);

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => {
                    debug!(
                        topic = %topic,
                        seq = event.seq,
                        event_type = %event.event,
                        "bridge worker: event received"
                    );
                    window.push(event);

                    if should_fire(&window) {
                        fire_and_record(
                            &topic, &lens, &window, &backend, &embedder, &corpus, &resolver,
                        )
                        .await;
                        window.events_since_fire = 0;
                        window.last_fire = Some(Instant::now());
                    }
                }
                Err(SseError::BadEvent(msg)) => {
                    warn!(
                        topic = %topic,
                        error = %msg,
                        "bridge worker: malformed event — skipping"
                    );
                }
                Err(e) => {
                    warn!(
                        topic = %topic,
                        error = %e,
                        "bridge worker: stream error — reconnecting in {}s",
                        RECONNECT_DELAY.as_secs()
                    );
                    break;
                }
            }
        }

        info!(
            topic = %topic,
            "bridge worker: stream ended — reconnecting in {}s",
            RECONNECT_DELAY.as_secs()
        );
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

async fn fire_and_record<B: ChatBackend>(
    topic: &str,
    lens: &Lens,
    window: &SessionWindow,
    backend: &B,
    embedder: &dyn Embedder,
    corpus: &Arc<Mutex<Corpus>>,
    resolver: &ModelResolver,
) {
    let snapshot: Vec<KatulongEvent> = window.events.iter().cloned().collect();
    let user_prompt = render_prompt(&snapshot);

    let worker = LensWorker::new(lens, backend, resolver);
    let actions = {
        let mut c = corpus.lock().await;
        match worker
            .run_with_tools(&user_prompt, &mut c, embedder, MAX_TOOL_ITERATIONS)
            .await
        {
            Ok(actions) => actions,
            Err(e) => {
                warn!(
                    topic = %topic,
                    error = %e,
                    "bridge worker: gemma fire failed"
                );
                return;
            }
        }
    };

    for a in &actions {
        match a {
            StructuralAction::Observe { content, tags } => {
                debug!(
                    topic = %topic,
                    content_len = content.len(),
                    tags = ?tags,
                    "bridge worker: observation recorded"
                );
            }
            other => {
                warn!(
                    topic = %topic,
                    action = ?other,
                    "bridge worker: structural verb produced but not yet dispatched (v1)"
                );
            }
        }
    }

    info!(
        topic = %topic,
        actions = actions.len(),
        "bridge worker: fire completed"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_fire_at_threshold() {
        let mut w = SessionWindow::new();
        for i in 0..FIRE_THRESHOLD {
            w.push(test_event(i as u64));
        }
        assert!(should_fire(&w));
    }

    #[test]
    fn should_not_fire_below_threshold() {
        let mut w = SessionWindow::new();
        for i in 0..(FIRE_THRESHOLD - 1) {
            w.push(test_event(i as u64));
        }
        assert!(!should_fire(&w));
    }

    #[test]
    fn window_cap_enforced() {
        let mut w = SessionWindow::new();
        for i in 0..(WINDOW_CAP + 50) {
            w.push(test_event(i as u64));
        }
        assert_eq!(w.events.len(), WINDOW_CAP);
        assert_eq!(w.events.front().unwrap().seq, 50);
    }

    #[test]
    fn resume_seq_tracks_highest() {
        let mut w = SessionWindow::new();
        assert_eq!(w.resume_seq(), 0);
        w.push(test_event(5));
        assert_eq!(w.resume_seq(), 6);
        w.push(test_event(10));
        assert_eq!(w.resume_seq(), 11);
    }

    #[test]
    fn render_prompt_formats_events() {
        let events = vec![
            test_event_typed(1, "tool-use"),
            test_event_typed(2, "permission-request"),
        ];
        let prompt = render_prompt(&events);
        assert!(prompt.contains("seq=1"));
        assert!(prompt.contains("tool-use"));
        assert!(prompt.contains("seq=2"));
        assert!(prompt.contains("permission-request"));
        assert!(prompt.contains("corpus.search"));
    }

    #[test]
    fn render_prompt_truncates_large_extra() {
        let mut extra = serde_json::Map::new();
        extra.insert(
            "big".to_string(),
            serde_json::Value::String("x".repeat(1000)),
        );
        let evt = KatulongEvent {
            seq: 1,
            event: "test".to_string(),
            timestamp: "2026-05-27T00:00:00Z".to_string(),
            session: None,
            extra,
        };
        let prompt = render_prompt(&[evt]);
        // The extra field should be truncated, not the full 1000+ chars
        assert!(prompt.len() < 1500);
    }

    #[test]
    fn bridge_lens_uses_fast_profile() {
        let lens = bridge_lens();
        assert_eq!(lens.name, "bridge");
        assert!(matches!(
            lens.model,
            ModelChoice::Profile(sipag_lens::Profile::Fast)
        ));
        assert!(matches!(lens.trigger, TriggerPolicy::Threshold));
    }

    fn test_event(seq: u64) -> KatulongEvent {
        test_event_typed(seq, "test-event")
    }

    fn test_event_typed(seq: u64, event_type: &str) -> KatulongEvent {
        KatulongEvent {
            seq,
            event: event_type.to_string(),
            timestamp: "2026-05-27T00:00:00Z".to_string(),
            session: Some("sess-1".to_string()),
            extra: serde_json::Map::new(),
        }
    }
}
