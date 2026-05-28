//! Lens-worker primitive — runtime that turns a lens definition
//! into structured observations + typed verb calls.
//!
//! Per `docs/architecture.md` §3 (Phase 1 #3) and `docs/architecture.md`
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
//! - [`LensWorker`] — runtime. Two entry points:
//!   - `run(input, corpus)` — one-shot chat → parse JSON → write
//!     Observes to the corpus with **empty embeddings** → return.
//!   - `run_with_tools(input, corpus, embedder, max_iterations)` —
//!     multi-turn loop that lets gemma call `corpus.search` /
//!     `corpus.expand` mid-prompt to retrieve prior observations
//!     before emitting its final structured output. Observes
//!     written through this path are **embedded** via the supplied
//!     `embedder`, so subsequent `corpus.search` calls can find
//!     them.
//! - [`execute_corpus_search`] / [`execute_corpus_expand`] —
//!   sipag-internal MCP-shape tool executors. Called by gemma
//!   mid-prompt via [`LensWorker::run_with_tools`]. NOT exposed to
//!   Claude (sipag is never an MCP server installed into project
//!   `.claude/` dirs — see `[[feedback-strict-layer-coupling]]`).
//!
//! ## What this crate does NOT own (boundary)
//!
//! - The scheduler / trigger dispatcher. `TriggerPolicy` is stored
//!   here; the loop that fires lens-workers on schedule or
//!   threshold-crossing lives in sipag's `serve` binary.
//! - UI dispatch for structural verbs. `LensWorker::run` returns
//!   `Vec<StructuralAction>`; the caller decides what to do with
//!   them (publish to broker, render in the web UI, etc.).

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ollama_bridge_client::{JobEndpoint, OllamaBridgeClient};
use serde::{Deserialize, Serialize};
use sipag_corpus::{Corpus, CorpusError, Embedder, SearchFilter};
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
/// defaults that match the dorky-robot stack (per architecture.md
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
    /// architecture.md (`Fast=gemma4:latest`, `Strong=gemma4:31b`,
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
    /// (per architecture.md Editing this text changes the worker's
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
    /// in architecture.md).
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
    /// much new content it hasn't yet processed (architecture.md
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
/// architecture.md "no per-classification recording verbs."
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
        /// (see architecture.md per-kind dedup windows).
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

// ── tool-calling protocol ──────────────────────────────────────────
//
// Sipag-internal MCP-shape. Lens-workers can call `corpus.search` and
// `corpus.expand` mid-prompt to retrieve prior observations from the
// corpus. The shape is a thin JSON envelope so any chat-capable model
// works — not gated on native tool-call support.
//
// NOT exposed to Claude (sipag is never an MCP server installed into
// project `.claude/` dirs — that would be a Demeter violation per
// `[[feedback-strict-layer-coupling]]`). These tools are called by
// gemma on sipag's side, reaching INTO sipag's own corpus.

/// One turn in a multi-turn chat history. Used by [`ChatBackend::chat_messages`]
/// and by [`LensWorker::run_with_tools`] to maintain context across
/// tool-call rounds. `role` is one of `"system"`, `"user"`,
/// `"assistant"`, `"tool"` — the Ollama `/api/chat` vocabulary.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// Wraps a [`ToolCall`] that the model emits as a non-terminal reply.
/// Parsed out of the model's text response by [`parse_tool_call`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCallEnvelope {
    pub tool_call: ToolCall,
}

/// The tool call itself. `name` is the dotted tool name
/// (`"corpus.search"` / `"corpus.expand"`); `arguments` is the
/// tool-specific JSON payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub name: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
}

/// Wraps a [`ToolResult`] that we send back to the model as a `tool`-
/// role message. Symmetric with [`ToolCallEnvelope`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolResultEnvelope {
    pub tool_result: ToolResult,
}

/// The tool result itself. `name` echoes the call's tool name;
/// `result` is the tool-specific JSON payload (see
/// [`execute_corpus_search`] / [`execute_corpus_expand`] for shapes).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolResult {
    pub name: String,
    pub result: serde_json::Value,
}

/// Recommended `max_iterations` for [`LensWorker::run_with_tools`].
/// Enough for search-then-expand-then-answer; low enough to catch
/// runaway loops. Callers may override.
pub const DEFAULT_TOOL_ITERATIONS: usize = 5;

/// Hard upper bound on `corpus.search`'s `top_k`. A model that asks
/// for an absurd value (e.g. millions) gets clamped to this. Local
/// corpora are small (thousands of items at most in v1); 100 is
/// already more than any sensible lens-worker reasoning needs.
pub const MAX_SEARCH_TOP_K: usize = 100;

/// Tool-protocol description appended to a lens's system prompt by
/// [`LensWorker::run_with_tools`]. Tells the model how to call the
/// two corpus tools and how to emit its terminal output.
///
/// Kept terse — Ollama-served local models follow short instructions
/// better than long ones, and the structured invariants (envelope
/// shape, terminal vs non-terminal reply) are what matter.
const TOOL_DOCS: &str = r#"

# Tools available

You have two sipag-internal tools for retrieving prior observations from the corpus:

- corpus.search(query, top_k?, filter_tags?, generation_at_most?, time_window?) — semantic search by cosine similarity. Returns up to top_k items, each with id, content, tags, timestamp, generation, score.
- corpus.expand(item_id) — fetch one item plus the items it was derived from (via source_refs). Returns { item, sources }.

To call a tool, reply with ONLY this JSON shape:
{"tool_call": {"name": "corpus.search", "arguments": {"query": "...", "top_k": 5}}}

You'll receive a tool_result and may call more tools or emit the final output.

# Final output

When ready, reply with ONLY this JSON shape:
{"actions": [...]}

Each action is one of: observe / suggest_stance / ask_human / propose_task.

Always reply with raw JSON — no surrounding prose."#;

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

    #[error("tool call failed: {0}")]
    ToolCall(String),

    #[error("tool-call iteration cap reached (max={max})")]
    ToolCallLimit { max: usize },

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type LensResult<T> = std::result::Result<T, LensError>;

// ── chat backend ───────────────────────────────────────────────────

/// Per-call sampling options the caller wants threaded through to
/// ollama's `/api/chat` `options` field. Each field is `Option` so
/// callers can request just temperature, or just num_predict, or
/// both. Backends that can't pass options through (test mocks, the
/// default-impl collapse) ignore the values silently — production
/// `BridgeChatBackend` includes whichever are `Some` in the request
/// body.
///
/// Added with §9 #9 (dispatch gate fold). The gate needs deterministic
/// JSON output from gemma; without options-passthrough, gemma defaults
/// to ~0.8 temperature and unbounded `num_predict`, which causes
/// structured-output drift. With `{temperature: 0.2, num_predict: 512}`
/// pinned, the gate's classification stays reliable.
#[derive(Debug, Clone, Default)]
pub struct ChatOptions {
    /// Sampling temperature. Lower = more deterministic (good for
    /// structured-JSON outputs); higher = more diverse. ollama
    /// default is ~0.8.
    pub temperature: Option<f32>,
    /// Maximum tokens to predict. `None` means use ollama default
    /// (`-1`, unbounded).
    pub num_predict: Option<u32>,
}

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

    /// Multi-turn chat. Used by [`LensWorker::run_with_tools`] so
    /// tool-call rounds preserve assistant + tool messages across
    /// iterations. The default impl collapses the message list back
    /// into one `chat()` call (concatenates system messages; prefixes
    /// non-system messages with their role) — fine for backends that
    /// don't support history natively. [`BridgeChatBackend`] overrides
    /// to pass `messages` straight through to Ollama's `/api/chat`.
    async fn chat_messages(&self, model: &str, messages: &[ChatMessage]) -> LensResult<String> {
        let mut system = String::new();
        let mut user = String::new();
        for m in messages {
            match m.role.as_str() {
                "system" => {
                    if !system.is_empty() {
                        system.push_str("\n\n");
                    }
                    system.push_str(&m.content);
                }
                role => {
                    if !user.is_empty() {
                        user.push_str("\n\n");
                    }
                    user.push_str(&format!("[{role}] {}", m.content));
                }
            }
        }
        self.chat(model, &system, &user).await
    }

    /// One-shot chat with per-call sampling options. Backends that
    /// can pass options to the underlying transport should override;
    /// the default impl ignores `options` and falls through to
    /// [`Self::chat`], which is fine for test mocks but means callers
    /// that genuinely need deterministic sampling MUST go through a
    /// backend that honors the options (e.g. [`BridgeChatBackend`]).
    ///
    /// Added with §9 #9 (dispatch gate fold) so the gate can pin its
    /// historical `{temperature: 0.2, num_predict: 512}` without
    /// forcing every existing single-arg call site to grow an unused
    /// options parameter.
    async fn chat_with_options(
        &self,
        model: &str,
        system: &str,
        user: &str,
        _options: &ChatOptions,
    ) -> LensResult<String> {
        self.chat(model, system, user).await
    }
}

/// Production [`ChatBackend`] — calls the bridge.
///
/// `Clone` because the underlying `OllamaBridgeClient` is `Clone`
/// (its `reqwest::Client` is Arc-internal) and `Duration` is `Copy`.
/// Callers that fan the backend out across the scheduler + the
/// dispatch gate + ad-hoc lens-workers benefit from cheap clones.
#[derive(Clone)]
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
        self.chat_messages(
            model,
            &[
                ChatMessage {
                    role: "system".into(),
                    content: system.to_string(),
                },
                ChatMessage {
                    role: "user".into(),
                    content: user.to_string(),
                },
            ],
        )
        .await
    }

    async fn chat_messages(&self, model: &str, messages: &[ChatMessage]) -> LensResult<String> {
        let json_messages: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
            .collect();
        let body = serde_json::json!({
            "model": model,
            "messages": json_messages,
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

    async fn chat_with_options(
        &self,
        model: &str,
        system: &str,
        user: &str,
        options: &ChatOptions,
    ) -> LensResult<String> {
        // Ollama's `/api/chat` accepts an `options` object alongside
        // `model` + `messages`. We only set fields the caller asked
        // for, so an empty ChatOptions yields no `options` key in the
        // body (identical to the plain `chat_messages` path).
        let mut options_obj = serde_json::Map::new();
        if let Some(t) = options.temperature {
            options_obj.insert("temperature".into(), serde_json::Value::from(t));
        }
        if let Some(n) = options.num_predict {
            options_obj.insert("num_predict".into(), serde_json::Value::from(n));
        }
        let mut body = serde_json::json!({
            "model": model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
        });
        if !options_obj.is_empty() {
            body.as_object_mut()
                .expect("constructed object above")
                .insert("options".into(), serde_json::Value::Object(options_obj));
        }
        let result = self
            .client
            .submit_and_wait(JobEndpoint::Chat, body, self.timeout)
            .await
            .map_err(|e| LensError::Bridge(e.to_string()))?;
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

// ── corpus tool executors ──────────────────────────────────────────

/// Execute a `corpus.search` tool call. Embeds the query via
/// `embedder`, then runs [`Corpus::search`] with the supplied filter.
/// Returns a `{ "items": [...] }` JSON payload.
///
/// `arguments` shape (only `query` is required):
/// ```json
/// {
///   "query": "...",
///   "top_k": 5,
///   "filter_tags": ["lens=foo"],
///   "generation_at_most": 1,
///   "time_window": { "after": "<rfc3339>", "before": "<rfc3339>" }
/// }
/// ```
///
/// Returned item shape: `{ id, content, tags, timestamp, generation, score }`.
pub async fn execute_corpus_search(
    arguments: &serde_json::Value,
    corpus: &Corpus,
    embedder: &dyn Embedder,
) -> LensResult<serde_json::Value> {
    let query = arguments
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or_else(|| LensError::ToolCall("corpus.search: missing 'query'".into()))?;
    let top_k = arguments
        .get("top_k")
        .and_then(|v| v.as_u64())
        .map(|n| (n as usize).min(MAX_SEARCH_TOP_K))
        .unwrap_or(5);

    let mut filter = SearchFilter::default();
    if let Some(tags) = arguments.get("filter_tags").and_then(|v| v.as_array()) {
        let tag_vec: Vec<String> = tags
            .iter()
            .filter_map(|t| t.as_str().map(String::from))
            .collect();
        // Empty list from the model means "no filter intended" rather
        // than sipag-corpus's "match nothing" (Some([])) semantics. The
        // model can't easily express the difference; the friendlier
        // interpretation is to drop the filter entirely.
        if !tag_vec.is_empty() {
            filter.tags = Some(tag_vec);
        }
    }
    if let Some(gen) = arguments.get("generation_at_most").and_then(|v| v.as_u64()) {
        let bounded = u8::try_from(gen).map_err(|_| {
            LensError::ToolCall(format!(
                "corpus.search: 'generation_at_most' must fit in u8, got {gen}"
            ))
        })?;
        filter.generation_at_most = Some(bounded);
    }
    if let Some(window) = arguments.get("time_window") {
        if let Some(after) = window.get("after").and_then(|v| v.as_str()) {
            filter.timestamp_after =
                Some(parse_rfc3339(after, "corpus.search", "time_window.after")?);
        }
        if let Some(before) = window.get("before").and_then(|v| v.as_str()) {
            filter.timestamp_before = Some(parse_rfc3339(
                before,
                "corpus.search",
                "time_window.before",
            )?);
        }
    }

    let query_embedding = embedder
        .embed(query)
        .await
        .map_err(|e| LensError::ToolCall(format!("corpus.search: embed failed: {e}")))?;
    let results = corpus.search(&query_embedding, &filter, top_k)?;
    let items: Vec<serde_json::Value> = results
        .into_iter()
        .map(|(score, item)| {
            serde_json::json!({
                "id": item.id,
                "content": item.content,
                "tags": item.tags,
                "timestamp": item.timestamp.to_rfc3339(),
                "generation": item.generation,
                "score": score,
            })
        })
        .collect();
    Ok(serde_json::json!({ "items": items }))
}

/// Execute a `corpus.expand` tool call. Returns the named item plus
/// the items it was derived from (one hop via `source_refs`).
///
/// `arguments` shape: `{ "item_id": <u64> }`.
///
/// Returned shape: `{ "item": { ... }, "sources": [ { ... } ] }`.
/// Source items missing from the corpus are silently skipped — an
/// invariant violation (broken derivation chain), but `expand` is a
/// read tool and not the right place to fail loudly.
pub fn execute_corpus_expand(
    arguments: &serde_json::Value,
    corpus: &Corpus,
) -> LensResult<serde_json::Value> {
    let item_id = arguments
        .get("item_id")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| LensError::ToolCall("corpus.expand: missing 'item_id'".into()))?;
    let item = corpus
        .get(item_id)
        .ok_or_else(|| LensError::ToolCall(format!("corpus.expand: no item with id {item_id}")))?;
    let sources: Vec<serde_json::Value> = item
        .source_refs
        .iter()
        .filter_map(|id| corpus.get(*id))
        .map(|src| {
            serde_json::json!({
                "id": src.id,
                "content": src.content,
                "tags": src.tags,
                "timestamp": src.timestamp.to_rfc3339(),
                "generation": src.generation,
            })
        })
        .collect();
    Ok(serde_json::json!({
        "item": {
            "id": item.id,
            "content": item.content,
            "tags": item.tags,
            "timestamp": item.timestamp.to_rfc3339(),
            "generation": item.generation,
            "source_refs": item.source_refs,
        },
        "sources": sources,
    }))
}

fn parse_rfc3339(s: &str, tool_name: &str, field: &str) -> LensResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| LensError::ToolCall(format!("{tool_name}: bad '{field}': {e}")))
}

/// Parse a model reply as a [`ToolCallEnvelope`]. Mirrors
/// [`LensWorker::parse_output`]'s tolerance — accepts either a clean
/// JSON object or JSON embedded in chatty prose (largest balanced
/// `{...}` block). Returns `None` when no parseable envelope is
/// found; callers fall back to [`LensWorker::parse_output`] or error.
pub fn parse_tool_call(raw: &str) -> Option<ToolCallEnvelope> {
    let trimmed = raw.trim();
    if let Ok(e) = serde_json::from_str::<ToolCallEnvelope>(trimmed) {
        return Some(e);
    }
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str::<ToolCallEnvelope>(&trimmed[start..=end]).ok()
}

// ── lens-worker runtime ────────────────────────────────────────────

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
    ///
    /// Observes are written with an EMPTY embedding — this method
    /// has no embedder. Use [`Self::run_with_tools`] (which takes
    /// an `&dyn Embedder` for the corpus tools) when you want
    /// Observes to land in the corpus searchable.
    ///
    /// **Mixed-mode caveat**: a corpus populated only through `run()`
    /// has stored_dim = 0 (the empty-embedding item dictates the
    /// per-corpus dimension invariant). Any later `corpus.search` call
    /// against that corpus will return [`CorpusError::DimensionMismatch`]
    /// because the embedder produces a non-zero-dim query. Pick one
    /// mode per corpus, or seed the corpus with a properly-embedded
    /// item before mixing.
    pub async fn run(&self, input: &str, corpus: &mut Corpus) -> LensResult<Vec<StructuralAction>> {
        let model = self.resolver.resolve(&self.lens.model);
        debug!(lens = %self.lens.name, model = %model, "lens-worker: chat start");
        let raw = self
            .backend
            .chat(&model, &self.lens.prompt_text, input)
            .await?;
        let output = Self::parse_output(&raw)?;
        self.finalize(output, corpus, None).await
    }

    /// Run a lens with mid-prompt access to `corpus.search` /
    /// `corpus.expand`. Loops up to `max_iterations`:
    ///
    /// - call the model with the current message history;
    /// - if the reply parses as a [`LensOutput`], that's terminal
    ///   — return its actions (Observes written to corpus, this
    ///   time with proper embeddings via `embedder`);
    /// - if it parses as a [`ToolCallEnvelope`], execute the
    ///   corresponding tool, append `assistant` + `tool` messages
    ///   to history, and loop;
    /// - if it parses as neither, return [`LensError::OutputParse`].
    ///
    /// Returns [`LensError::ToolCallLimit`] if the cap is reached
    /// without a terminal output. [`DEFAULT_TOOL_ITERATIONS`] (`5`)
    /// is a reasonable default for `max_iterations`.
    ///
    /// **Tool errors abort the loop, not the tool round.** If
    /// `corpus.search` returns a `CorpusError::DimensionMismatch`
    /// (operator-config issue) or `corpus.expand` is called with an
    /// unknown id, the error bubbles straight out — the model never
    /// sees a "search failed, retry with different args" path. This
    /// is deliberate: tool failures here are upstream-config or
    /// model-confusion bugs, not signals the model can recover from
    /// without operator intervention.
    pub async fn run_with_tools(
        &self,
        input: &str,
        corpus: &mut Corpus,
        embedder: &dyn Embedder,
        max_iterations: usize,
    ) -> LensResult<Vec<StructuralAction>> {
        let model = self.resolver.resolve(&self.lens.model);
        debug!(
            lens = %self.lens.name,
            model = %model,
            max_iterations,
            "lens-worker: tool-calling start"
        );
        let system_prompt = format!("{}{TOOL_DOCS}", self.lens.prompt_text);
        let mut messages = vec![
            ChatMessage {
                role: "system".into(),
                content: system_prompt,
            },
            ChatMessage {
                role: "user".into(),
                content: input.to_string(),
            },
        ];
        for iteration in 0..max_iterations {
            let raw = self.backend.chat_messages(&model, &messages).await?;
            // Tool call FIRST. LensOutput's `actions` field is
            // `#[serde(default)]`, so a tool_call envelope happens to
            // parse as an empty LensOutput — checking tool_call first
            // disambiguates correctly.
            if let Some(envelope) = parse_tool_call(&raw) {
                let tool_name = envelope.tool_call.name.clone();
                debug!(
                    lens = %self.lens.name,
                    iteration,
                    tool = %tool_name,
                    "lens-worker: executing tool"
                );
                let result_value = match tool_name.as_str() {
                    "corpus.search" => {
                        execute_corpus_search(&envelope.tool_call.arguments, corpus, embedder)
                            .await?
                    }
                    "corpus.expand" => {
                        execute_corpus_expand(&envelope.tool_call.arguments, corpus)?
                    }
                    other => {
                        return Err(LensError::ToolCall(format!("unknown tool: {other}")));
                    }
                };
                let tool_result_json = serde_json::to_string(&ToolResultEnvelope {
                    tool_result: ToolResult {
                        name: tool_name,
                        result: result_value,
                    },
                })
                .map_err(|e| LensError::ToolCall(format!("serialize tool_result: {e}")))?;
                messages.push(ChatMessage {
                    role: "assistant".into(),
                    content: raw,
                });
                messages.push(ChatMessage {
                    role: "tool".into(),
                    content: tool_result_json,
                });
                continue;
            }
            // Not a tool call — must be terminal output (or garbage).
            if let Ok(output) = Self::parse_output(&raw) {
                debug!(
                    lens = %self.lens.name,
                    iterations = iteration + 1,
                    "lens-worker: terminal output reached"
                );
                return self.finalize(output, corpus, Some(embedder)).await;
            }
            return Err(LensError::OutputParse(format!(
                "neither tool_call nor LensOutput in reply: {raw}"
            )));
        }
        Err(LensError::ToolCallLimit {
            max: max_iterations,
        })
    }

    /// Shared post-processing for both [`Self::run`] and
    /// [`Self::run_with_tools`]:
    ///
    /// - auto-tag each `Observe` with `lens=<name>` (idempotent);
    /// - write Observes to `corpus`; embeds them via `embedder` if
    ///   one was provided, otherwise writes content-only items
    ///   (back-compat with [`Self::run`]'s pre-tool-calling
    ///   contract);
    /// - return ALL actions to the caller (corpus write is
    ///   side-effectful but not gating).
    async fn finalize(
        &self,
        output: LensOutput,
        corpus: &mut Corpus,
        embedder: Option<&dyn Embedder>,
    ) -> LensResult<Vec<StructuralAction>> {
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
        for action in &tagged {
            if let StructuralAction::Observe { content, tags } = action {
                // Loud-warn when an Observe lands with no embedder —
                // the on-disk item will be persisted with an empty
                // embedding and therefore invisible to `corpus.search`.
                // Catches accidental use of plain `run()` for content
                // the caller intends to search later.
                if embedder.is_none() {
                    warn!(
                        lens = %self.lens.name,
                        "lens-worker: writing Observe with empty embedding; \
                         subsequent corpus.search will not find this item. \
                         Use run_with_tools() with an embedder for searchable Observes."
                    );
                }
                // Call .embed() directly (rather than corpus.add_text)
                // because add_text takes `E: Embedder` with implicit
                // Sized bound; routing through .embed() + .add()
                // accepts `&dyn Embedder` without forcing sipag-corpus
                // to relax that signature.
                let embedding = match embedder {
                    Some(e) => match e.embed(content).await {
                        Ok(v) => v,
                        Err(err) => {
                            // Embed failed — preserve the content with
                            // an empty embedding rather than dropping
                            // it. Search won't find it (same as the
                            // None-embedder path) but the observation
                            // isn't silently lost.
                            warn!(
                                lens = %self.lens.name,
                                error = %err,
                                "lens-worker: embedder failed; writing Observe with empty embedding"
                            );
                            Vec::new()
                        }
                    },
                    None => Vec::new(),
                };
                if let Err(e) = corpus
                    .add(content.clone(), embedding, tags.clone(), Vec::new(), 0)
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
        // Per architecture.md open-question note, the tentative
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

    // ── tool-calling feature tests ──────────────────────────────────
    //
    // These tests pin the wrappers' PROMISE to lens-workers:
    //
    // - corpus.search / corpus.expand return well-shaped results that
    //   gemma can reason against;
    // - the multi-turn loop preserves history across tool rounds and
    //   terminates when the model emits a LensOutput;
    // - the iteration cap catches runaway loops;
    // - unknown tools and malformed replies surface as errors instead
    //   of silent no-ops;
    // - run_with_tools embeds Observes so subsequent corpus.search can
    //   find them (contrast: plain `run()` writes empty-embedding items,
    //   intentionally not searchable — pinned below).
    //
    // Wire-format details (exact JSON byte layout, role string values)
    // are NOT asserted directly — implementation tweaks that preserve
    // the above contract should NOT churn these tests.

    use std::collections::VecDeque;

    /// Replays a scripted list of model replies, capturing the message
    /// history passed to each call so tests can assert on it. Forces
    /// callers through `chat_messages` so tool-calling rounds use the
    /// real multi-turn path.
    struct ScriptedBackend {
        replies: Mutex<VecDeque<String>>,
        captured: Mutex<Vec<Vec<ChatMessage>>>,
    }

    impl ScriptedBackend {
        fn new<I, S>(replies: I) -> Self
        where
            I: IntoIterator<Item = S>,
            S: Into<String>,
        {
            Self {
                replies: Mutex::new(replies.into_iter().map(Into::into).collect()),
                captured: Mutex::new(Vec::new()),
            }
        }

        async fn calls(&self) -> usize {
            self.captured.lock().await.len()
        }

        async fn nth_history(&self, n: usize) -> Vec<ChatMessage> {
            self.captured.lock().await[n].clone()
        }
    }

    #[async_trait]
    impl ChatBackend for ScriptedBackend {
        async fn chat(&self, _: &str, _: &str, _: &str) -> LensResult<String> {
            // run_with_tools always routes through chat_messages; this
            // arm is only here to satisfy the trait.
            Err(LensError::Bridge(
                "ScriptedBackend doesn't implement single-turn chat".into(),
            ))
        }

        async fn chat_messages(
            &self,
            _model: &str,
            messages: &[ChatMessage],
        ) -> LensResult<String> {
            self.captured.lock().await.push(messages.to_vec());
            let mut q = self.replies.lock().await;
            q.pop_front()
                .ok_or_else(|| LensError::Bridge("ScriptedBackend ran out of replies".into()))
        }
    }

    /// Deterministic hash-derived embedder. Same text → same vector;
    /// distinct text → distinct vector. Enough to drive search ranking
    /// without spinning up ollama.
    struct FakeEmbedder {
        dim: usize,
    }

    #[async_trait]
    impl Embedder for FakeEmbedder {
        async fn embed(&self, text: &str) -> sipag_corpus::CorpusResult<Vec<f32>> {
            let bytes = text.as_bytes();
            let mut out = Vec::with_capacity(self.dim);
            for i in 0..self.dim {
                let v = if bytes.is_empty() {
                    0.0
                } else {
                    bytes[i % bytes.len()] as f32 / 255.0
                };
                out.push(v);
            }
            Ok(out)
        }
    }

    fn search_call(query: &str) -> String {
        serde_json::json!({
            "tool_call": {
                "name": "corpus.search",
                "arguments": { "query": query, "top_k": 3 },
            }
        })
        .to_string()
    }

    fn expand_call(item_id: u64) -> String {
        serde_json::json!({
            "tool_call": {
                "name": "corpus.expand",
                "arguments": { "item_id": item_id },
            }
        })
        .to_string()
    }

    fn lens_output_with_observe(content: &str) -> String {
        serde_json::json!({
            "actions": [
                { "verb": "observe", "content": content, "tags": [] }
            ]
        })
        .to_string()
    }

    async fn populate_two_items(corpus: &mut Corpus, embedder: &FakeEmbedder) -> (u64, u64) {
        let a = corpus
            .add_text(
                embedder,
                "alpha — about cats".into(),
                vec!["topic=cats".into()],
                vec![],
                0,
            )
            .await
            .unwrap();
        let b = corpus
            .add_text(
                embedder,
                "beta — about dogs".into(),
                vec!["topic=dogs".into()],
                vec![],
                0,
            )
            .await
            .unwrap();
        (a, b)
    }

    // ── direct executor tests ───────────────────────────────────────

    #[tokio::test]
    async fn execute_corpus_search_returns_items_ranked_by_similarity() {
        let dir = TempDir::new().unwrap();
        let mut corpus = Corpus::open(dir.path()).await.unwrap();
        let embedder = FakeEmbedder { dim: 4 };
        populate_two_items(&mut corpus, &embedder).await;
        // Query identical to "alpha — about cats" → top result.
        let args = serde_json::json!({ "query": "alpha — about cats", "top_k": 2 });
        let result = execute_corpus_search(&args, &corpus, &embedder)
            .await
            .unwrap();
        let items = result.get("items").and_then(|v| v.as_array()).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0].get("content").and_then(|v| v.as_str()),
            Some("alpha — about cats"),
            "exact-match query should be top-ranked"
        );
        // Each item carries the documented shape.
        for item in items {
            assert!(item.get("id").is_some());
            assert!(item.get("content").is_some());
            assert!(item.get("tags").is_some());
            assert!(item.get("timestamp").is_some());
            assert!(item.get("generation").is_some());
            assert!(item.get("score").is_some());
        }
    }

    #[tokio::test]
    async fn execute_corpus_search_filter_tags_narrow_results() {
        let dir = TempDir::new().unwrap();
        let mut corpus = Corpus::open(dir.path()).await.unwrap();
        let embedder = FakeEmbedder { dim: 4 };
        populate_two_items(&mut corpus, &embedder).await;
        let args = serde_json::json!({
            "query": "anything",
            "top_k": 5,
            "filter_tags": ["topic=cats"],
        });
        let result = execute_corpus_search(&args, &corpus, &embedder)
            .await
            .unwrap();
        let items = result.get("items").and_then(|v| v.as_array()).unwrap();
        assert_eq!(items.len(), 1, "filter_tags should exclude non-matching");
        assert_eq!(
            items[0].get("content").and_then(|v| v.as_str()),
            Some("alpha — about cats")
        );
    }

    #[tokio::test]
    async fn execute_corpus_search_missing_query_errors() {
        let dir = TempDir::new().unwrap();
        let corpus = Corpus::open(dir.path()).await.unwrap();
        let embedder = FakeEmbedder { dim: 4 };
        let args = serde_json::json!({ "top_k": 3 });
        let err = execute_corpus_search(&args, &corpus, &embedder)
            .await
            .unwrap_err();
        assert!(matches!(err, LensError::ToolCall(_)));
    }

    #[tokio::test]
    async fn execute_corpus_search_bad_time_window_errors() {
        let dir = TempDir::new().unwrap();
        let corpus = Corpus::open(dir.path()).await.unwrap();
        let embedder = FakeEmbedder { dim: 4 };
        let args = serde_json::json!({
            "query": "x",
            "time_window": { "after": "not-a-date" },
        });
        let err = execute_corpus_search(&args, &corpus, &embedder)
            .await
            .unwrap_err();
        match err {
            LensError::ToolCall(msg) => assert!(msg.contains("time_window.after")),
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn execute_corpus_expand_returns_item_and_sources() {
        let dir = TempDir::new().unwrap();
        let mut corpus = Corpus::open(dir.path()).await.unwrap();
        let embedder = FakeEmbedder { dim: 4 };
        let a = corpus
            .add_text(&embedder, "source-A".into(), vec![], vec![], 0)
            .await
            .unwrap();
        let b = corpus
            .add_text(&embedder, "source-B".into(), vec![], vec![], 0)
            .await
            .unwrap();
        let derived = corpus
            .add_text(&embedder, "derivation X".into(), vec![], vec![a, b], 1)
            .await
            .unwrap();
        let args = serde_json::json!({ "item_id": derived });
        let result = execute_corpus_expand(&args, &corpus).unwrap();
        // Item itself.
        assert_eq!(
            result
                .get("item")
                .and_then(|v| v.get("id"))
                .and_then(|v| v.as_u64()),
            Some(derived)
        );
        // Sources include BOTH parents.
        let sources = result.get("sources").and_then(|v| v.as_array()).unwrap();
        let source_ids: Vec<u64> = sources
            .iter()
            .filter_map(|v| v.get("id").and_then(|i| i.as_u64()))
            .collect();
        assert_eq!(sources.len(), 2);
        assert!(source_ids.contains(&a));
        assert!(source_ids.contains(&b));
    }

    #[tokio::test]
    async fn execute_corpus_expand_missing_item_id_errors() {
        let dir = TempDir::new().unwrap();
        let corpus = Corpus::open(dir.path()).await.unwrap();
        let args = serde_json::json!({});
        let err = execute_corpus_expand(&args, &corpus).unwrap_err();
        assert!(matches!(err, LensError::ToolCall(_)));
    }

    #[tokio::test]
    async fn execute_corpus_expand_unknown_item_errors() {
        let dir = TempDir::new().unwrap();
        let corpus = Corpus::open(dir.path()).await.unwrap();
        let args = serde_json::json!({ "item_id": 999 });
        let err = execute_corpus_expand(&args, &corpus).unwrap_err();
        assert!(matches!(err, LensError::ToolCall(_)));
    }

    #[test]
    fn parse_tool_call_accepts_clean_envelope() {
        let raw = r#"{"tool_call":{"name":"corpus.search","arguments":{"query":"x"}}}"#;
        let e = parse_tool_call(raw).expect("should parse clean JSON");
        assert_eq!(e.tool_call.name, "corpus.search");
    }

    #[test]
    fn parse_tool_call_extracts_envelope_from_prose() {
        let raw = "Sure, calling the tool:\n\n{\"tool_call\":{\"name\":\"corpus.expand\",\"arguments\":{\"item_id\":42}}}\n\nLet me know.";
        let e = parse_tool_call(raw).expect("should parse JSON embedded in prose");
        assert_eq!(e.tool_call.name, "corpus.expand");
    }

    #[test]
    fn parse_tool_call_returns_none_on_garbage() {
        assert!(parse_tool_call("I have no idea").is_none());
    }

    // ── multi-turn run_with_tools tests ─────────────────────────────

    #[tokio::test]
    async fn run_with_tools_terminates_immediately_on_lens_output() {
        // Model emits final output on the first reply, skipping any
        // tool rounds. run_with_tools must return cleanly — the tool
        // docs appended to the system prompt should not break parsing.
        let dir = TempDir::new().unwrap();
        let mut corpus = Corpus::open(dir.path()).await.unwrap();
        let embedder = FakeEmbedder { dim: 4 };
        let lens = test_lens("immediate", "you are a watcher");
        let resolver = ModelResolver::with_defaults();
        let backend = ScriptedBackend::new([lens_output_with_observe("first-turn answer")]);
        let worker = LensWorker::new(&lens, &backend, &resolver);
        let actions = worker
            .run_with_tools("look at this", &mut corpus, &embedder, 5)
            .await
            .unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(backend.calls().await, 1);
    }

    #[tokio::test]
    async fn run_with_tools_executes_search_and_returns_results_to_model() {
        // Multi-turn proof: model emits a search tool_call on turn 1,
        // receives results as a `tool` message in the history, and
        // emits its final output on turn 2.
        let dir = TempDir::new().unwrap();
        let mut corpus = Corpus::open(dir.path()).await.unwrap();
        let embedder = FakeEmbedder { dim: 4 };
        populate_two_items(&mut corpus, &embedder).await;
        let lens = test_lens("searcher", "you are a searcher");
        let resolver = ModelResolver::with_defaults();
        let backend = ScriptedBackend::new([
            search_call("alpha — about cats"),
            lens_output_with_observe("synthesis after search"),
        ]);
        let worker = LensWorker::new(&lens, &backend, &resolver);
        let actions = worker
            .run_with_tools("input", &mut corpus, &embedder, 5)
            .await
            .unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(backend.calls().await, 2);
        // Second turn's history should include the search tool_result.
        let history = backend.nth_history(1).await;
        let tool_msg = history
            .iter()
            .find(|m| m.role == "tool")
            .expect("tool turn appended after search");
        assert!(tool_msg.content.contains("corpus.search"));
        assert!(tool_msg.content.contains("alpha"));
    }

    #[tokio::test]
    async fn run_with_tools_executes_expand_then_emits_output() {
        let dir = TempDir::new().unwrap();
        let mut corpus = Corpus::open(dir.path()).await.unwrap();
        let embedder = FakeEmbedder { dim: 4 };
        let src = corpus
            .add_text(&embedder, "source".into(), vec![], vec![], 0)
            .await
            .unwrap();
        let derived = corpus
            .add_text(&embedder, "derived".into(), vec![], vec![src], 1)
            .await
            .unwrap();
        let lens = test_lens("expander", "you are an expander");
        let resolver = ModelResolver::with_defaults();
        let backend = ScriptedBackend::new([
            expand_call(derived),
            lens_output_with_observe("walked the chain"),
        ]);
        let worker = LensWorker::new(&lens, &backend, &resolver);
        worker
            .run_with_tools("input", &mut corpus, &embedder, 5)
            .await
            .unwrap();
        assert_eq!(backend.calls().await, 2);
        let history = backend.nth_history(1).await;
        let tool_msg = history.iter().find(|m| m.role == "tool").unwrap();
        assert!(tool_msg.content.contains("\"sources\""));
        assert!(tool_msg.content.contains("source"));
    }

    #[tokio::test]
    async fn run_with_tools_caps_iterations() {
        // Always returns a tool call — should hit ToolCallLimit.
        let dir = TempDir::new().unwrap();
        let mut corpus = Corpus::open(dir.path()).await.unwrap();
        let embedder = FakeEmbedder { dim: 4 };
        let lens = test_lens("loopy", "you are loopy");
        let resolver = ModelResolver::with_defaults();
        let replies: Vec<String> = (0..10).map(|_| search_call("x")).collect();
        let backend = ScriptedBackend::new(replies);
        let worker = LensWorker::new(&lens, &backend, &resolver);
        let err = worker
            .run_with_tools("input", &mut corpus, &embedder, 3)
            .await
            .unwrap_err();
        match err {
            LensError::ToolCallLimit { max } => assert_eq!(max, 3),
            other => panic!("expected ToolCallLimit, got {other:?}"),
        }
        // Confirmed cap stopped at 3 (not 10).
        assert_eq!(backend.calls().await, 3);
    }

    #[tokio::test]
    async fn run_with_tools_unknown_tool_errors() {
        let dir = TempDir::new().unwrap();
        let mut corpus = Corpus::open(dir.path()).await.unwrap();
        let embedder = FakeEmbedder { dim: 4 };
        let lens = test_lens("weird", "");
        let resolver = ModelResolver::with_defaults();
        let bogus = serde_json::json!({
            "tool_call": { "name": "corpus.purge", "arguments": {} }
        })
        .to_string();
        let backend = ScriptedBackend::new([bogus]);
        let worker = LensWorker::new(&lens, &backend, &resolver);
        let err = worker
            .run_with_tools("input", &mut corpus, &embedder, 5)
            .await
            .unwrap_err();
        match err {
            LensError::ToolCall(msg) => assert!(msg.contains("corpus.purge")),
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_with_tools_garbage_reply_errors() {
        let dir = TempDir::new().unwrap();
        let mut corpus = Corpus::open(dir.path()).await.unwrap();
        let embedder = FakeEmbedder { dim: 4 };
        let lens = test_lens("garbage", "");
        let resolver = ModelResolver::with_defaults();
        let backend = ScriptedBackend::new(["I refuse to comply"]);
        let worker = LensWorker::new(&lens, &backend, &resolver);
        let err = worker
            .run_with_tools("input", &mut corpus, &embedder, 5)
            .await
            .unwrap_err();
        assert!(matches!(err, LensError::OutputParse(_)));
    }

    #[tokio::test]
    async fn run_with_tools_embeds_observes_so_subsequent_search_finds_them() {
        // The whole reason corpus.search exists: prior Observes should
        // be retrievable. run_with_tools must embed them — `run()` does
        // not (see test below).
        let dir = TempDir::new().unwrap();
        let mut corpus = Corpus::open(dir.path()).await.unwrap();
        let embedder = FakeEmbedder { dim: 4 };
        let lens = test_lens("embedder-lens", "");
        let resolver = ModelResolver::with_defaults();
        let backend = ScriptedBackend::new([lens_output_with_observe("a brand new observation")]);
        let worker = LensWorker::new(&lens, &backend, &resolver);
        worker
            .run_with_tools("input", &mut corpus, &embedder, 5)
            .await
            .unwrap();
        let item = corpus.get(1).expect("observe should have been written");
        assert!(
            !item.embedding.is_empty(),
            "run_with_tools should embed Observes via the supplied embedder"
        );
        assert_eq!(item.embedding.len(), 4);
    }

    #[tokio::test]
    async fn run_without_tools_writes_unembedded_observe_for_back_compat() {
        // Pin the prior contract of `run()`: writes Observes with empty
        // embedding. Changing it later should be a conscious decision.
        let dir = TempDir::new().unwrap();
        let mut corpus = Corpus::open(dir.path()).await.unwrap();
        let lens = test_lens("plain", "");
        let resolver = ModelResolver::with_defaults();
        let backend = CannedChatBackend::new(lens_output_with_observe("plain observe"));
        let worker = LensWorker::new(&lens, &backend, &resolver);
        worker.run("input", &mut corpus).await.unwrap();
        let item = corpus.get(1).unwrap();
        assert!(item.embedding.is_empty(), "run() must not embed");
    }

    #[tokio::test]
    async fn run_with_tools_history_grows_with_assistant_and_tool_turns() {
        // After K tool rounds, the next history snapshot should contain
        // system + user + (assistant + tool) * K = 2 + 2K messages.
        let dir = TempDir::new().unwrap();
        let mut corpus = Corpus::open(dir.path()).await.unwrap();
        let embedder = FakeEmbedder { dim: 4 };
        let lens = test_lens("counter", "");
        let resolver = ModelResolver::with_defaults();
        let backend = ScriptedBackend::new([
            search_call("q"),
            search_call("q"),
            lens_output_with_observe("done"),
        ]);
        let worker = LensWorker::new(&lens, &backend, &resolver);
        worker
            .run_with_tools("input", &mut corpus, &embedder, 5)
            .await
            .unwrap();
        // Snapshot 0: system + user → 2 messages.
        assert_eq!(backend.nth_history(0).await.len(), 2);
        // Snapshot 1: + (assistant + tool) → 4 messages.
        assert_eq!(backend.nth_history(1).await.len(), 4);
        // Snapshot 2: + (assistant + tool) → 6 messages.
        assert_eq!(backend.nth_history(2).await.len(), 6);
    }

    #[tokio::test]
    async fn run_with_tools_system_prompt_includes_lens_text_and_tool_docs() {
        // The lens's prompt_text + the tool protocol description must
        // both reach the backend as the system message — neither
        // should silently drop.
        let dir = TempDir::new().unwrap();
        let mut corpus = Corpus::open(dir.path()).await.unwrap();
        let embedder = FakeEmbedder { dim: 4 };
        let lens = test_lens("doc-check", "MY LENS PROMPT");
        let resolver = ModelResolver::with_defaults();
        let backend = ScriptedBackend::new([lens_output_with_observe("done")]);
        let worker = LensWorker::new(&lens, &backend, &resolver);
        worker
            .run_with_tools("input", &mut corpus, &embedder, 3)
            .await
            .unwrap();
        let history = backend.nth_history(0).await;
        let sys = history.iter().find(|m| m.role == "system").unwrap();
        assert!(sys.content.contains("MY LENS PROMPT"));
        assert!(sys.content.contains("corpus.search"));
        assert!(sys.content.contains("corpus.expand"));
    }

    #[tokio::test]
    async fn default_chat_messages_collapses_to_chat() {
        // The trait default impl of chat_messages should fall back to
        // chat() for backends that only implement single-turn. Verify
        // the collapsed system + user composition.
        let backend = CannedChatBackend::new("{}");
        let messages = vec![
            ChatMessage {
                role: "system".into(),
                content: "S1".into(),
            },
            ChatMessage {
                role: "user".into(),
                content: "U1".into(),
            },
            ChatMessage {
                role: "assistant".into(),
                content: "A1".into(),
            },
            ChatMessage {
                role: "tool".into(),
                content: "T1".into(),
            },
        ];
        backend.chat_messages("any-model", &messages).await.unwrap();
        let sys = backend.captured_system.lock().await.clone().unwrap();
        let usr = backend.captured_user.lock().await.clone().unwrap();
        assert_eq!(sys, "S1");
        assert!(usr.contains("[user] U1"));
        assert!(usr.contains("[assistant] A1"));
        assert!(usr.contains("[tool] T1"));
    }

    // ── round-1 review-fix coverage ─────────────────────────────────

    #[tokio::test]
    async fn execute_corpus_search_generation_at_most_narrows_to_raw_observations() {
        // Mix generation 0 + generation 1 items; filter to gen=0.
        let dir = TempDir::new().unwrap();
        let mut corpus = Corpus::open(dir.path()).await.unwrap();
        let embedder = FakeEmbedder { dim: 4 };
        corpus
            .add_text(&embedder, "raw observation".into(), vec![], vec![], 0)
            .await
            .unwrap();
        corpus
            .add_text(&embedder, "derived synthesis".into(), vec![], vec![1], 1)
            .await
            .unwrap();
        let args = serde_json::json!({
            "query": "anything",
            "top_k": 5,
            "generation_at_most": 0,
        });
        let result = execute_corpus_search(&args, &corpus, &embedder)
            .await
            .unwrap();
        let items = result.get("items").and_then(|v| v.as_array()).unwrap();
        assert_eq!(
            items.len(),
            1,
            "generation_at_most=0 should exclude derived"
        );
        assert_eq!(
            items[0].get("content").and_then(|v| v.as_str()),
            Some("raw observation")
        );
    }

    #[tokio::test]
    async fn execute_corpus_search_generation_at_most_oversized_errors() {
        // Model passing a value > u8::MAX should error rather than
        // silently wrap to a wrong filter.
        let dir = TempDir::new().unwrap();
        let corpus = Corpus::open(dir.path()).await.unwrap();
        let embedder = FakeEmbedder { dim: 4 };
        let args = serde_json::json!({
            "query": "x",
            "generation_at_most": 9999,
        });
        let err = execute_corpus_search(&args, &corpus, &embedder)
            .await
            .unwrap_err();
        match err {
            LensError::ToolCall(msg) => assert!(msg.contains("generation_at_most")),
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn execute_corpus_search_top_k_clamped_to_max() {
        // top_k > MAX_SEARCH_TOP_K should be clamped, not allocate
        // absurd amounts. Verify via the returned items length (bounded
        // by both top_k and corpus size).
        let dir = TempDir::new().unwrap();
        let mut corpus = Corpus::open(dir.path()).await.unwrap();
        let embedder = FakeEmbedder { dim: 4 };
        for i in 0..3 {
            corpus
                .add_text(&embedder, format!("item-{i}"), vec![], vec![], 0)
                .await
                .unwrap();
        }
        let args = serde_json::json!({
            "query": "anything",
            "top_k": 10_000_000_u64,
        });
        let result = execute_corpus_search(&args, &corpus, &embedder)
            .await
            .unwrap();
        let items = result.get("items").and_then(|v| v.as_array()).unwrap();
        assert_eq!(
            items.len(),
            3,
            "should return all 3 items (clamped top_k still > corpus size)"
        );
    }

    #[tokio::test]
    async fn execute_corpus_search_empty_filter_tags_treated_as_no_filter() {
        // The model passing `"filter_tags": []` is friendlier as
        // "no filter intended" than sipag-corpus's "match nothing"
        // semantics. Verify: results returned, not silently zero.
        let dir = TempDir::new().unwrap();
        let mut corpus = Corpus::open(dir.path()).await.unwrap();
        let embedder = FakeEmbedder { dim: 4 };
        populate_two_items(&mut corpus, &embedder).await;
        let args = serde_json::json!({
            "query": "alpha — about cats",
            "top_k": 5,
            "filter_tags": [],
        });
        let result = execute_corpus_search(&args, &corpus, &embedder)
            .await
            .unwrap();
        let items = result.get("items").and_then(|v| v.as_array()).unwrap();
        assert_eq!(items.len(), 2, "empty filter_tags must not match-nothing");
    }

    #[tokio::test]
    async fn execute_corpus_search_bad_time_window_before_errors() {
        let dir = TempDir::new().unwrap();
        let corpus = Corpus::open(dir.path()).await.unwrap();
        let embedder = FakeEmbedder { dim: 4 };
        let args = serde_json::json!({
            "query": "x",
            "time_window": { "before": "still-not-a-date" },
        });
        let err = execute_corpus_search(&args, &corpus, &embedder)
            .await
            .unwrap_err();
        match err {
            LensError::ToolCall(msg) => {
                assert!(msg.contains("time_window.before"));
                assert!(msg.contains("corpus.search"));
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn parse_tool_call_returns_none_on_braces_without_valid_json() {
        // The `{ ... }` extraction path must not panic on braces that
        // wrap garbage. Without this test the prose-with-real-envelope
        // case alone is the only guard on that branch.
        assert!(parse_tool_call("{ not json at all }").is_none());
        assert!(parse_tool_call("{tool_call: missing quotes}").is_none());
        assert!(parse_tool_call("{{nested braces no json}}").is_none());
    }

    #[tokio::test]
    async fn run_with_tools_propagates_tool_executor_error_mid_loop() {
        // When `corpus.expand` is called with an unknown item_id,
        // execute_corpus_expand returns LensError::ToolCall; verify
        // run_with_tools bubbles it out cleanly (does not absorb /
        // re-wrap / loop on it).
        let dir = TempDir::new().unwrap();
        let mut corpus = Corpus::open(dir.path()).await.unwrap();
        let embedder = FakeEmbedder { dim: 4 };
        let lens = test_lens("error-bubble", "");
        let resolver = ModelResolver::with_defaults();
        let backend = ScriptedBackend::new([
            expand_call(9999),
            lens_output_with_observe("should not be reached"),
        ]);
        let worker = LensWorker::new(&lens, &backend, &resolver);
        let err = worker
            .run_with_tools("input", &mut corpus, &embedder, 5)
            .await
            .unwrap_err();
        match err {
            LensError::ToolCall(msg) => assert!(msg.contains("9999")),
            other => panic!("expected ToolCall, got {other:?}"),
        }
        // Backend should have been called exactly once (the expand
        // failed before the loop could call again).
        assert_eq!(backend.calls().await, 1);
    }

    #[tokio::test]
    async fn run_with_tools_observe_then_search_round_trips() {
        // Headline contract: an Observe written via run_with_tools
        // becomes discoverable through a subsequent corpus.search
        // tool_call from another run_with_tools invocation. This is
        // the end-to-end proof that the tool path makes Observes
        // searchable.
        let dir = TempDir::new().unwrap();
        let mut corpus = Corpus::open(dir.path()).await.unwrap();
        let embedder = FakeEmbedder { dim: 4 };
        let resolver = ModelResolver::with_defaults();

        // Run #1 — model emits an Observe directly.
        let lens_a = test_lens("writer", "you write observations");
        let backend_a =
            ScriptedBackend::new([lens_output_with_observe("there are seven cats in the room")]);
        LensWorker::new(&lens_a, &backend_a, &resolver)
            .run_with_tools("look around", &mut corpus, &embedder, 5)
            .await
            .unwrap();

        // Run #2 — model searches for the prior observation, then
        // emits a synthesis Observe referring to it. Verify the search
        // tool_result contains the prior observation by content.
        let lens_b = test_lens("synthesizer", "you synthesize observations");
        let backend_b = ScriptedBackend::new([
            search_call("there are seven cats in the room"),
            lens_output_with_observe("counted: seven feline residents"),
        ]);
        LensWorker::new(&lens_b, &backend_b, &resolver)
            .run_with_tools("synthesize", &mut corpus, &embedder, 5)
            .await
            .unwrap();
        // Run #2's second turn should have received the prior Observe.
        let history = backend_b.nth_history(1).await;
        let tool_msg = history.iter().find(|m| m.role == "tool").unwrap();
        assert!(
            tool_msg
                .content
                .contains("there are seven cats in the room"),
            "search tool_result should include prior Observe content; got: {}",
            tool_msg.content
        );
    }

    #[tokio::test]
    async fn finalize_falls_back_to_empty_embedding_when_embedder_fails() {
        // FailingEmbedder always errors. run_with_tools should still
        // PERSIST the Observe (content + tags) with an empty embedding
        // — not silently drop it. Search won't find it (same as the
        // None-embedder path), but the observation isn't lost.
        struct FailingEmbedder;
        #[async_trait]
        impl Embedder for FailingEmbedder {
            async fn embed(&self, _: &str) -> sipag_corpus::CorpusResult<Vec<f32>> {
                Err(sipag_corpus::CorpusError::Embed("forced failure".into()))
            }
        }
        let dir = TempDir::new().unwrap();
        let mut corpus = Corpus::open(dir.path()).await.unwrap();
        let lens = test_lens("fallback", "");
        let resolver = ModelResolver::with_defaults();
        let backend = ScriptedBackend::new([lens_output_with_observe("important note")]);
        let worker = LensWorker::new(&lens, &backend, &resolver);
        worker
            .run_with_tools("input", &mut corpus, &FailingEmbedder, 3)
            .await
            .unwrap();
        let item = corpus
            .get(1)
            .expect("Observe should be written with empty embedding on embedder failure");
        assert_eq!(item.content, "important note");
        assert!(item.embedding.is_empty(), "fallback writes empty embedding");
    }

    #[test]
    fn default_tool_iterations_is_recommended_value() {
        // Pin the recommendation so a future tweak forces a docs sync.
        assert_eq!(DEFAULT_TOOL_ITERATIONS, 5);
    }

    #[test]
    fn max_search_top_k_pinned() {
        // Pin the cap so changes are deliberate (and visible in tests).
        assert_eq!(MAX_SEARCH_TOP_K, 100);
    }
}
