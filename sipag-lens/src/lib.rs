//! Lens-worker primitive — runtime that turns a lens definition
//! into structured observations + typed verb calls.
//!
//! Per `docs/modules.md` §3 (Phase 1 #3) and `docs/extraction-plan.md`
//! §3: this is the abstraction every Steering entry becomes a
//! configuration of, plus project-meta and ad-hoc workers. The
//! bridge worker (gemma watching katulong events) is just the first
//! instance.
//!
//! ## What this crate owns
//!
//! - [`Lens`] — the value object. Name, prompt text, source, model
//!   choice, trigger policy, retired flag.
//! - [`LensSource`] — what kind of lens: a Steering entry, a
//!   project-meta lens, or an ad-hoc one.
//! - [`ModelChoice`] + [`Profile`] — per-lens model selection,
//!   semantically tier-based with environment-specific concrete
//!   model names resolved via [`ModelResolver`].
//! - [`TriggerPolicy`] — schedule / threshold / model-decide.
//!   Stored on the lens; the scheduler lives elsewhere (out of v1
//!   scope; this crate provides the types so a future scheduler
//!   crate can consume them).
//! - [`StructuralAction`] — typed verb output. `Observe` is the
//!   workhorse (free-form text into the corpus); `SuggestStance` /
//!   `AskHuman` / `ProposeTask` are the three structural verbs that
//!   drive UI affordances in Steering.
//! - [`ModelResolver`] — reads `~/.sipag/models.toml` (or a
//!   caller-supplied path) and resolves `ModelChoice` → concrete
//!   model name. Ships with built-in defaults that match the
//!   dorky-robot stack.
//! - [`LensWorker`] — runtime. One method: `run(input) -> Result<Vec<StructuralAction>>`.
//!   Composes prompt → calls bridge → parses structured JSON →
//!   writes any `Observe` actions to the corpus → returns the
//!   structural verbs for the caller to dispatch.
//!
//! ## What this crate does NOT own (boundary)
//!
//! - The scheduler / trigger dispatcher. `TriggerPolicy` is stored
//!   here; the loop that fires lens-workers on schedule or
//!   threshold-crossing lives in sipag's `serve` binary.
//! - UI dispatch for structural verbs. `LensWorker::run` returns
//!   `Vec<StructuralAction>`; the caller decides what to do with
//!   them (publish to broker, render in the web UI, etc.).
//! - `corpus.search` / `corpus.expand` MCP tool wrappers. The
//!   underlying primitives live in `sipag-corpus`; the
//!   MCP-shape gemma-callable wrapper is a follow-up.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ollama_bridge_client::{JobEndpoint, OllamaBridgeClient};
use serde::{Deserialize, Serialize};
use sipag_corpus::{Corpus, CorpusError};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, warn};

// ── model selection ────────────────────────────────────────────────

/// Per-lens model selection. `Default` falls back to a global
/// (typically the `OLLAMA_MODEL` env var or the resolver's built-in
/// default); `Named` pins a specific model; `Profile` decouples
/// the lens definition from concrete model names so the same lens
/// runs on different machines with different resolver maps.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ModelChoice {
    #[default]
    Default,
    Named(String),
    Profile(Profile),
}

/// Semantic tier rather than a specific model. Maps to concrete
/// model names per environment via [`ModelResolver`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    /// Bridge-tier workers: high-frequency, threshold-driven,
    /// must feel real-time (~5-7s/call budget). Maps to small fast
    /// local models.
    Fast,
    /// Derivation-tier workers: strategic, meta-cognitive,
    /// pattern-spotter. Runs daily or on threshold;
    /// quality > latency (~20-30s/call OK). Maps to large local
    /// models.
    Strong,
    /// Workers that read diffs / source / commits. Maps to
    /// code-tuned models.
    CodeAware,
}

/// Resolves [`ModelChoice`] → concrete model name. Reads
/// `~/.sipag/models.toml` if present; otherwise uses built-in
/// defaults that match the dorky-robot stack (per modules.md
/// §10's tentative-defaults note).
///
/// `models.toml` shape:
///
/// ```toml
/// default = "gemma4:latest"
///
/// [profiles]
/// fast = "gemma4:latest"
/// strong = "gemma4:31b"
/// code_aware = "qwen2.5-coder:7b"
/// ```
#[derive(Debug, Clone)]
pub struct ModelResolver {
    default_model: String,
    profile_map: HashMap<Profile, String>,
}

impl ModelResolver {
    /// Build a resolver with the built-in defaults baked into
    /// modules.md §10 (`Fast=gemma4:latest`, `Strong=gemma4:31b`,
    /// `CodeAware=qwen2.5-coder:7b`). No file I/O.
    pub fn with_defaults() -> Self {
        let mut profile_map = HashMap::new();
        profile_map.insert(Profile::Fast, "gemma4:latest".to_string());
        profile_map.insert(Profile::Strong, "gemma4:31b".to_string());
        profile_map.insert(Profile::CodeAware, "qwen2.5-coder:7b".to_string());
        Self {
            default_model: "gemma4:latest".to_string(),
            profile_map,
        }
    }

    /// Load from `~/.sipag/models.toml` if the file exists; falls
    /// back to [`Self::with_defaults`] if not. Returns an error
    /// only if the file exists but is malformed.
    pub fn load() -> LensResult<Self> {
        let sipag_dir = sipag_dir_path()?;
        Self::load_from(&sipag_dir.join("models.toml"))
    }

    /// Load from a specific path. If the file doesn't exist,
    /// returns [`Self::with_defaults`]. If it exists but is
    /// malformed, returns [`LensError::ModelConfig`].
    pub fn load_from(path: &Path) -> LensResult<Self> {
        if !path.exists() {
            return Ok(Self::with_defaults());
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| LensError::ModelConfig(format!("read {}: {e}", path.display())))?;
        let parsed: ModelsTomlFile = toml::from_str(&content)
            .map_err(|e| LensError::ModelConfig(format!("parse {}: {e}", path.display())))?;
        // Start from defaults, then overlay any explicit values.
        let mut resolver = Self::with_defaults();
        if let Some(d) = parsed.default {
            resolver.default_model = d;
        }
        if let Some(profiles) = parsed.profiles {
            if let Some(s) = profiles.fast {
                resolver.profile_map.insert(Profile::Fast, s);
            }
            if let Some(s) = profiles.strong {
                resolver.profile_map.insert(Profile::Strong, s);
            }
            if let Some(s) = profiles.code_aware {
                resolver.profile_map.insert(Profile::CodeAware, s);
            }
        }
        Ok(resolver)
    }

    /// Resolve a `ModelChoice` to a concrete model name.
    pub fn resolve(&self, choice: &ModelChoice) -> String {
        match choice {
            ModelChoice::Default => self.default_model.clone(),
            ModelChoice::Named(name) => name.clone(),
            ModelChoice::Profile(p) => self
                .profile_map
                .get(p)
                .cloned()
                .unwrap_or_else(|| self.default_model.clone()),
        }
    }
}

fn sipag_dir_path() -> LensResult<std::path::PathBuf> {
    if let Ok(dir) = std::env::var("SIPAG_DIR") {
        return Ok(std::path::PathBuf::from(dir));
    }
    let home = std::env::var("HOME").map_err(|_| LensError::ModelConfig("HOME not set".into()))?;
    Ok(std::path::PathBuf::from(home).join(".sipag"))
}

#[derive(Debug, Deserialize)]
struct ModelsTomlFile {
    #[serde(default)]
    default: Option<String>,
    #[serde(default)]
    profiles: Option<ProfilesTable>,
}

#[derive(Debug, Deserialize)]
struct ProfilesTable {
    #[serde(default)]
    fast: Option<String>,
    #[serde(default)]
    strong: Option<String>,
    #[serde(default, rename = "code_aware")]
    code_aware: Option<String>,
}

// ── lens ───────────────────────────────────────────────────────────

/// One lens — a worker definition. Created from a Steering entry
/// (Objective / KR / Standing / Idea), a project-meta declaration,
/// or an ad-hoc UI action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lens {
    /// Stable lens identifier. For Steering-entry lenses, typically
    /// `objective/<id>` or `kr/<id>`.
    pub name: String,
    /// Free-form text — the system prompt of the gemma invocation.
    /// THE load-bearing seam between Steering and Experimentation
    /// (per modules.md §6). Editing this text changes the worker's
    /// behavior.
    pub prompt_text: String,
    /// Origin. Determines lifecycle (Steering-entry lenses retire
    /// when the entry is closed/deleted; ad-hoc lenses expire on a
    /// promote-or-expire policy).
    pub source: LensSource,
    /// Per-lens model selection. Falls back via
    /// [`ModelResolver::resolve`].
    #[serde(default)]
    pub model: ModelChoice,
    /// Trigger policy — when this worker fires. Stored on the lens;
    /// the dispatcher lives outside this crate (sipag's `serve`).
    #[serde(default)]
    pub trigger: TriggerPolicy,
    /// Soft-retired flag — kept on disk for diwa-indexable history
    /// but not fired by the dispatcher. Per
    /// `[[feedback-deprecate-with-rationale]]`.
    #[serde(default)]
    pub retired: bool,
}

/// Where a lens came from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LensSource {
    /// Steering entry — Objective / KR / Standing / Idea. The
    /// `id` is the entry's id within sipag-core::board.
    SteeringEntry { id: String },
    /// Project-meta — pattern-spotter, meta-cognitive,
    /// strategic-cross-cutting. Not tied to a single Steering
    /// entry.
    ProjectMeta { name: String },
    /// Ad-hoc — user-created via the web UI for short-lived
    /// hypotheses. Expires per
    /// `[[feedback-deprecate-with-rationale]]` if not promoted to
    /// a Steering entry within N days.
    AdHoc {
        creator: String,
        created: DateTime<Utc>,
    },
}

/// When a lens-worker fires. v1 stores the types; the scheduler
/// loop that consumes them is a follow-up.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TriggerPolicy {
    /// Run periodically at this interval. Used for derivation-tier
    /// workers (strategic, meta-cognitive — see §3 trigger discussion
    /// in modules.md).
    Schedule {
        #[serde(with = "duration_secs")]
        interval: Duration,
    },
    /// Run when a domain-specific threshold crosses (e.g. N
    /// new katulong events on a `claude/<uuid>` topic; M new
    /// commits on a project). Threshold details are domain-specific
    /// and lens-supplied — the scheduler hands off to the
    /// lens-worker, which checks its own condition.
    Threshold,
    /// Worker decides for itself whether to re-run based on how
    /// much new content it hasn't yet processed (modules.md §10
    /// open question — formalize once telemetry is in).
    ModelDecide,
}

impl Default for TriggerPolicy {
    fn default() -> Self {
        // Schedule(1h) is a safe default — high-frequency workers
        // override to Threshold or a shorter Schedule.
        Self::Schedule {
            interval: Duration::from_secs(3600),
        }
    }
}

/// Serde adapter for Duration ↔ seconds (TOML doesn't have a
/// native duration type).
mod duration_secs {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;
    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(d.as_secs())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let secs = u64::deserialize(d)?;
        Ok(Duration::from_secs(secs))
    }
}

// ── verbs ──────────────────────────────────────────────────────────

/// What a lens-worker produced after one gemma invocation. The
/// workhorse is `Observe` (free-form text, becomes a CorpusItem);
/// the other three drive UI affordances in Steering. Per
/// modules.md §3, "no per-classification recording verbs."
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "verb", rename_all = "snake_case")]
pub enum StructuralAction {
    /// Free-form observation. Written to the corpus, never to the
    /// human surface directly. `tags` typically includes
    /// `lens=<name>` plus context tags (`kr_ref`, `session`, etc.).
    Observe {
        content: String,
        #[serde(default)]
        tags: Vec<String>,
    },
    /// Lens proposes a KR stance change. Caller dispatches to the
    /// Steering UI.
    SuggestStance {
        kr_ref: String,
        stance: String,
        reason: String,
    },
    /// Lens has a question that needs human input. Caller surfaces
    /// in the KR sidebar.
    AskHuman {
        question: String,
        /// `permission-style` / `progress-check` / `is-this-KR-still-alive`
        /// (see modules.md §3 per-kind dedup windows).
        kind: String,
    },
    /// Lens suggests a new task. Caller can accept (create task)
    /// or reject.
    ProposeTask { title: String, body: String },
}

/// Top-level wrapper the lens emits — a list of structural
/// actions, in case one invocation produces multiple verbs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct LensOutput {
    #[serde(default)]
    pub actions: Vec<StructuralAction>,
}

// ── errors ─────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum LensError {
    #[error("model config: {0}")]
    ModelConfig(String),

    #[error("bridge call failed: {0}")]
    Bridge(String),

    #[error("could not parse lens output as JSON: {0}")]
    OutputParse(String),

    #[error("corpus error: {0}")]
    Corpus(#[from] CorpusError),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type LensResult<T> = std::result::Result<T, LensError>;

// ── lens-worker runtime ────────────────────────────────────────────

/// The chat backend a [`LensWorker`] talks to. Decoupled from
/// `OllamaBridgeClient` directly so tests can inject a mock that
/// returns canned JSON without spinning up a real bridge.
///
/// Production impl is [`BridgeChatBackend`] (default cargo feature
/// already on through `ollama-bridge-client`).
#[async_trait]
pub trait ChatBackend: Send + Sync {
    /// One-shot chat. `system` is the lens's prompt_text; `user`
    /// is the input the lens-worker is reasoning about. Returns
    /// the assistant's textual reply (whatever the model
    /// produces; lens-workers parse it as JSON via
    /// [`LensWorker::parse_output`]).
    async fn chat(&self, model: &str, system: &str, user: &str) -> LensResult<String>;
}

/// Production [`ChatBackend`] — calls the bridge.
pub struct BridgeChatBackend {
    client: OllamaBridgeClient,
    timeout: Duration,
}

impl BridgeChatBackend {
    /// 60-second default timeout per chat call.
    pub fn new(client: OllamaBridgeClient) -> Self {
        Self {
            client,
            timeout: Duration::from_secs(60),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

#[async_trait]
impl ChatBackend for BridgeChatBackend {
    async fn chat(&self, model: &str, system: &str, user: &str) -> LensResult<String> {
        let body = serde_json::json!({
            "model": model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
        });
        let result = self
            .client
            .submit_and_wait(JobEndpoint::Chat, body, self.timeout)
            .await
            .map_err(|e| LensError::Bridge(e.to_string()))?;
        // Ollama's /api/chat response shape:
        //   { "message": { "role": "assistant", "content": "..." } }
        let content = result
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| {
                LensError::Bridge(format!("no message.content in bridge response: {result}"))
            })?;
        Ok(content.to_string())
    }
}

/// One run of a lens. Composes the bridge call, parses the output,
/// writes any `Observe` actions to the corpus, and returns the
/// structural actions for the caller to dispatch.
pub struct LensWorker<'a, B: ChatBackend> {
    pub lens: &'a Lens,
    pub backend: &'a B,
    pub resolver: &'a ModelResolver,
}

impl<'a, B: ChatBackend> LensWorker<'a, B> {
    pub fn new(lens: &'a Lens, backend: &'a B, resolver: &'a ModelResolver) -> Self {
        Self {
            lens,
            backend,
            resolver,
        }
    }

    /// Run one lens invocation. The `input` is the user prompt
    /// (typically a render of the corpus query + the new content
    /// the lens is looking at). The lens's `prompt_text` becomes
    /// the system prompt.
    ///
    /// On success: parses the model's reply as a [`LensOutput`]
    /// (JSON), writes any `Observe` actions to `corpus` with
    /// `lens=<name>` tag auto-added, returns the actions for the
    /// caller. Other structural verbs (`SuggestStance`, `AskHuman`,
    /// `ProposeTask`) are returned but NOT written to the corpus —
    /// the caller dispatches them to UI surfaces.
    pub async fn run(&self, input: &str, corpus: &mut Corpus) -> LensResult<Vec<StructuralAction>> {
        let model = self.resolver.resolve(&self.lens.model);
        debug!(lens = %self.lens.name, model = %model, "lens-worker: chat start");
        let raw = self
            .backend
            .chat(&model, &self.lens.prompt_text, input)
            .await?;
        let output = Self::parse_output(&raw)?;
        // Auto-tag observes with the lens name so search-by-lens
        // works without callers having to add it. Other verbs flow
        // through unchanged.
        let mut tagged = Vec::with_capacity(output.actions.len());
        for action in output.actions {
            let action = match action {
                StructuralAction::Observe { content, mut tags } => {
                    let lens_tag = format!("lens={}", self.lens.name);
                    if !tags.iter().any(|t| t == &lens_tag) {
                        tags.push(lens_tag);
                    }
                    StructuralAction::Observe { content, tags }
                }
                other => other,
            };
            tagged.push(action);
        }
        // Write Observe actions to the corpus immediately. Embedding
        // is the caller's concern (this crate doesn't know which
        // embedder model the corpus was populated with).
        //
        // The crate writes with an empty embedding vector — callers
        // that want vector search must use a `LensWorker` variant
        // that takes an embedder OR re-add the items via
        // `Corpus::add_text(embedder, ...)`. This split matches the
        // sipag-corpus design: the storage layer accepts
        // pre-embedded items; embedder composition is the caller's.
        //
        // For v1, write content-only items so the lens-worker
        // pipeline is end-to-end without requiring embedder
        // wiring. Vector search over lens outputs is a follow-up.
        for action in &tagged {
            if let StructuralAction::Observe { content, tags } = action {
                if let Err(e) = corpus
                    .add(content.clone(), Vec::new(), tags.clone(), Vec::new(), 0)
                    .await
                {
                    warn!(
                        lens = %self.lens.name,
                        error = %e,
                        "lens-worker: corpus write failed; continuing"
                    );
                }
            }
        }
        Ok(tagged)
    }

    /// Parse a model reply into a [`LensOutput`]. Tolerates two
    /// shapes:
    /// 1. The whole reply is the JSON object (`{"actions":[...]}`).
    /// 2. The JSON is embedded inside text — extract the largest
    ///    `{...}` block.
    ///
    /// Returns [`LensError::OutputParse`] if neither shape produces
    /// valid JSON.
    pub fn parse_output(raw: &str) -> LensResult<LensOutput> {
        let trimmed = raw.trim();
        if let Ok(o) = serde_json::from_str::<LensOutput>(trimmed) {
            return Ok(o);
        }
        // Try extracting the largest balanced `{...}` block (handles
        // models that prefix/suffix prose around the JSON).
        if let Some(start) = trimmed.find('{') {
            // Find a matching `}` walking back from the end.
            if let Some(end) = trimmed.rfind('}') {
                if end > start {
                    let candidate = &trimmed[start..=end];
                    if let Ok(o) = serde_json::from_str::<LensOutput>(candidate) {
                        return Ok(o);
                    }
                }
            }
        }
        Err(LensError::OutputParse(format!(
            "no parseable JSON in model reply: {raw}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sipag_corpus::Corpus;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::Mutex;

    // ── canned chat backend for tests ───────────────────────────────

    struct CannedChatBackend {
        reply: String,
        // Capture what the lens asked so tests can assert on it.
        captured_model: Arc<Mutex<Option<String>>>,
        captured_system: Arc<Mutex<Option<String>>>,
        captured_user: Arc<Mutex<Option<String>>>,
    }

    impl CannedChatBackend {
        fn new(reply: impl Into<String>) -> Self {
            Self {
                reply: reply.into(),
                captured_model: Arc::new(Mutex::new(None)),
                captured_system: Arc::new(Mutex::new(None)),
                captured_user: Arc::new(Mutex::new(None)),
            }
        }
    }

    #[async_trait]
    impl ChatBackend for CannedChatBackend {
        async fn chat(&self, model: &str, system: &str, user: &str) -> LensResult<String> {
            *self.captured_model.lock().await = Some(model.to_string());
            *self.captured_system.lock().await = Some(system.to_string());
            *self.captured_user.lock().await = Some(user.to_string());
            Ok(self.reply.clone())
        }
    }

    struct FailingBackend;

    #[async_trait]
    impl ChatBackend for FailingBackend {
        async fn chat(&self, _m: &str, _s: &str, _u: &str) -> LensResult<String> {
            Err(LensError::Bridge("simulated bridge failure".into()))
        }
    }

    fn test_lens(name: &str, prompt: &str) -> Lens {
        Lens {
            name: name.into(),
            prompt_text: prompt.into(),
            source: LensSource::SteeringEntry { id: "kr/1".into() },
            model: ModelChoice::Default,
            trigger: TriggerPolicy::default(),
            retired: false,
        }
    }

    // ── feature-requirement tests ───────────────────────────────────

    #[test]
    fn resolver_defaults_match_modules_md_tentative() {
        // Per modules.md §10 open-question note, the tentative
        // defaults are: Fast=gemma4:latest, Strong=gemma4:31b,
        // CodeAware=qwen2.5-coder:7b. Pin these so a future tweak
        // forces a docs+code sync.
        let r = ModelResolver::with_defaults();
        assert_eq!(
            r.resolve(&ModelChoice::Profile(Profile::Fast)),
            "gemma4:latest"
        );
        assert_eq!(
            r.resolve(&ModelChoice::Profile(Profile::Strong)),
            "gemma4:31b"
        );
        assert_eq!(
            r.resolve(&ModelChoice::Profile(Profile::CodeAware)),
            "qwen2.5-coder:7b"
        );
        assert_eq!(r.resolve(&ModelChoice::Default), "gemma4:latest");
    }

    #[test]
    fn resolver_named_passes_through_verbatim() {
        let r = ModelResolver::with_defaults();
        assert_eq!(
            r.resolve(&ModelChoice::Named("my-custom-model:v2".into())),
            "my-custom-model:v2"
        );
    }

    #[test]
    fn resolver_missing_models_toml_falls_back_to_defaults() {
        let dir = TempDir::new().unwrap();
        // No models.toml present.
        let r = ModelResolver::load_from(&dir.path().join("models.toml")).unwrap();
        assert_eq!(
            r.resolve(&ModelChoice::Profile(Profile::Fast)),
            "gemma4:latest"
        );
    }

    #[test]
    fn resolver_loads_overrides_from_models_toml() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("models.toml");
        std::fs::write(
            &path,
            r#"
default = "custom-default:v1"

[profiles]
fast = "alpaca:7b"
code_aware = "starcoder:15b"
"#,
        )
        .unwrap();
        let r = ModelResolver::load_from(&path).unwrap();
        // Overridden.
        assert_eq!(r.resolve(&ModelChoice::Default), "custom-default:v1");
        assert_eq!(r.resolve(&ModelChoice::Profile(Profile::Fast)), "alpaca:7b");
        assert_eq!(
            r.resolve(&ModelChoice::Profile(Profile::CodeAware)),
            "starcoder:15b"
        );
        // NOT overridden — falls back to built-in default.
        assert_eq!(
            r.resolve(&ModelChoice::Profile(Profile::Strong)),
            "gemma4:31b"
        );
    }

    #[test]
    fn resolver_malformed_models_toml_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("models.toml");
        std::fs::write(&path, "this is = not [valid toml").unwrap();
        let err = ModelResolver::load_from(&path).unwrap_err();
        match err {
            LensError::ModelConfig(_) => {}
            other => panic!("expected ModelConfig, got {other:?}"),
        }
    }

    #[test]
    fn parse_output_accepts_clean_json() {
        let raw = r#"{"actions":[{"verb":"observe","content":"hello","tags":["x"]}]}"#;
        let o = LensWorker::<CannedChatBackend>::parse_output(raw).unwrap();
        assert_eq!(o.actions.len(), 1);
        match &o.actions[0] {
            StructuralAction::Observe { content, tags } => {
                assert_eq!(content, "hello");
                assert_eq!(tags, &vec!["x".to_string()]);
            }
            other => panic!("expected Observe, got {other:?}"),
        }
    }

    #[test]
    fn parse_output_extracts_json_from_chatty_prose() {
        // Models sometimes prefix/suffix prose around the JSON
        // ("Sure! Here's the result:" ... "Let me know if you need
        // more!"). The parser tolerates this by extracting the
        // largest `{...}` block.
        let raw = r#"Sure! Here is the output:

        {"actions":[{"verb":"observe","content":"the dispatch worked","tags":[]}]}

        Let me know if you'd like more analysis."#;
        let o = LensWorker::<CannedChatBackend>::parse_output(raw).unwrap();
        assert_eq!(o.actions.len(), 1);
    }

    #[test]
    fn parse_output_rejects_no_json() {
        let raw = "I'm sorry, I can't help with that.";
        let err = LensWorker::<CannedChatBackend>::parse_output(raw).unwrap_err();
        match err {
            LensError::OutputParse(_) => {}
            other => panic!("expected OutputParse, got {other:?}"),
        }
    }

    #[test]
    fn parse_output_round_trips_all_four_verbs() {
        // Pin the wire format for all four structural verbs so a
        // future schema tweak forces a deliberate decision.
        let raw = r#"{"actions":[
            {"verb":"observe","content":"x","tags":[]},
            {"verb":"suggest_stance","kr_ref":"kr/1","stance":"yellow","reason":"risk"},
            {"verb":"ask_human","question":"approve?","kind":"permission-style"},
            {"verb":"propose_task","title":"do thing","body":"because"}
        ]}"#;
        let o = LensWorker::<CannedChatBackend>::parse_output(raw).unwrap();
        assert_eq!(o.actions.len(), 4);
        assert!(matches!(o.actions[0], StructuralAction::Observe { .. }));
        assert!(matches!(
            o.actions[1],
            StructuralAction::SuggestStance { .. }
        ));
        assert!(matches!(o.actions[2], StructuralAction::AskHuman { .. }));
        assert!(matches!(o.actions[3], StructuralAction::ProposeTask { .. }));
    }

    #[tokio::test]
    async fn lens_worker_writes_observe_to_corpus_and_returns_all_actions() {
        let dir = TempDir::new().unwrap();
        let mut corpus = Corpus::open(dir.path()).await.unwrap();
        let lens = test_lens("test-lens", "you are a helpful observer");
        let resolver = ModelResolver::with_defaults();
        let backend = CannedChatBackend::new(
            r#"{"actions":[
                {"verb":"observe","content":"saw a thing","tags":["session=s1"]},
                {"verb":"suggest_stance","kr_ref":"kr/1","stance":"green","reason":"on track"}
            ]}"#,
        );
        let worker = LensWorker::new(&lens, &backend, &resolver);
        let actions = worker
            .run("here is what happened", &mut corpus)
            .await
            .unwrap();
        // Both actions returned to the caller.
        assert_eq!(actions.len(), 2);
        // Corpus gained the observe (1 item).
        assert_eq!(corpus.len(), 1);
        let item = corpus.get(1).unwrap();
        assert_eq!(item.content, "saw a thing");
        // session tag preserved AND lens tag auto-added.
        assert!(item.tags.contains(&"session=s1".to_string()));
        assert!(item.tags.contains(&"lens=test-lens".to_string()));
        // suggest_stance NOT written to corpus.
        assert_eq!(
            corpus.iter().count(),
            1,
            "only Observe actions should land in the corpus"
        );
    }

    #[tokio::test]
    async fn lens_worker_resolves_profile_and_calls_backend_with_concrete_model() {
        let dir = TempDir::new().unwrap();
        let mut corpus = Corpus::open(dir.path()).await.unwrap();
        let mut lens = test_lens("strong-lens", "system prompt");
        lens.model = ModelChoice::Profile(Profile::Strong);
        let resolver = ModelResolver::with_defaults();
        let backend = CannedChatBackend::new(r#"{"actions":[]}"#);
        let worker = LensWorker::new(&lens, &backend, &resolver);
        worker.run("input", &mut corpus).await.unwrap();
        let model = backend.captured_model.lock().await.clone();
        // Strong → gemma4:31b per the resolver's defaults.
        assert_eq!(model.as_deref(), Some("gemma4:31b"));
    }

    #[tokio::test]
    async fn lens_worker_passes_prompt_text_as_system_and_input_as_user() {
        let dir = TempDir::new().unwrap();
        let mut corpus = Corpus::open(dir.path()).await.unwrap();
        let lens = test_lens("seam-lens", "YOU ARE A SYSTEM PROMPT");
        let resolver = ModelResolver::with_defaults();
        let backend = CannedChatBackend::new(r#"{"actions":[]}"#);
        let worker = LensWorker::new(&lens, &backend, &resolver);
        worker.run("the user query", &mut corpus).await.unwrap();
        assert_eq!(
            backend.captured_system.lock().await.as_deref(),
            Some("YOU ARE A SYSTEM PROMPT")
        );
        assert_eq!(
            backend.captured_user.lock().await.as_deref(),
            Some("the user query")
        );
    }

    #[tokio::test]
    async fn lens_worker_propagates_backend_failure() {
        let dir = TempDir::new().unwrap();
        let mut corpus = Corpus::open(dir.path()).await.unwrap();
        let lens = test_lens("failing-lens", "");
        let resolver = ModelResolver::with_defaults();
        let backend = FailingBackend;
        let worker = LensWorker::new(&lens, &backend, &resolver);
        let err = worker.run("input", &mut corpus).await.unwrap_err();
        match err {
            LensError::Bridge(msg) => assert!(msg.contains("simulated")),
            other => panic!("expected Bridge, got {other:?}"),
        }
        // Corpus untouched on backend failure.
        assert_eq!(corpus.len(), 0);
    }

    #[tokio::test]
    async fn lens_worker_observe_without_lens_tag_gets_it_auto_added() {
        // Model omitted the `lens=<name>` tag; worker adds it.
        let dir = TempDir::new().unwrap();
        let mut corpus = Corpus::open(dir.path()).await.unwrap();
        let lens = test_lens("auto-tag-lens", "");
        let resolver = ModelResolver::with_defaults();
        let backend =
            CannedChatBackend::new(r#"{"actions":[{"verb":"observe","content":"x","tags":[]}]}"#);
        let worker = LensWorker::new(&lens, &backend, &resolver);
        let actions = worker.run("input", &mut corpus).await.unwrap();
        if let StructuralAction::Observe { tags, .. } = &actions[0] {
            assert!(tags.contains(&"lens=auto-tag-lens".to_string()));
        } else {
            panic!("expected Observe");
        }
    }

    #[tokio::test]
    async fn lens_worker_observe_with_existing_lens_tag_not_duplicated() {
        let dir = TempDir::new().unwrap();
        let mut corpus = Corpus::open(dir.path()).await.unwrap();
        let lens = test_lens("dedup-lens", "");
        let resolver = ModelResolver::with_defaults();
        let backend = CannedChatBackend::new(
            r#"{"actions":[{"verb":"observe","content":"x","tags":["lens=dedup-lens"]}]}"#,
        );
        let worker = LensWorker::new(&lens, &backend, &resolver);
        let actions = worker.run("input", &mut corpus).await.unwrap();
        if let StructuralAction::Observe { tags, .. } = &actions[0] {
            let count = tags
                .iter()
                .filter(|t| t.as_str() == "lens=dedup-lens")
                .count();
            assert_eq!(count, 1, "lens tag should not be duplicated");
        } else {
            panic!("expected Observe");
        }
    }

    #[test]
    fn trigger_policy_serializes_round_trip() {
        // The dispatcher (out of v1 scope) will consume these from
        // disk. Pin the serialization shape so the future
        // dispatcher's parser doesn't bit-rot.
        let cases = vec![
            TriggerPolicy::Schedule {
                interval: Duration::from_secs(3600),
            },
            TriggerPolicy::Threshold,
            TriggerPolicy::ModelDecide,
        ];
        for tp in cases {
            let s = serde_json::to_string(&tp).unwrap();
            let back: TriggerPolicy = serde_json::from_str(&s).unwrap();
            assert_eq!(back, tp);
        }
    }

    #[test]
    fn lens_source_round_trips_all_variants() {
        let cases = vec![
            LensSource::SteeringEntry { id: "kr/42".into() },
            LensSource::ProjectMeta {
                name: "pattern-spotter".into(),
            },
            LensSource::AdHoc {
                creator: "felix".into(),
                created: Utc::now(),
            },
        ];
        for src in cases {
            let s = serde_json::to_string(&src).unwrap();
            let back: LensSource = serde_json::from_str(&s).unwrap();
            assert_eq!(back, src);
        }
    }
}
