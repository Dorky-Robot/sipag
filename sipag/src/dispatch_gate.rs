//! Dispatch gate — classify a katulong session's current state.
//!
//! Sipag dispatches by typing an agent command into a katulong tmux
//! pane. The pane may be in any of a dozen states the dispatcher
//! can't usefully enumerate up front: logged in and idle, sitting on
//! a "Run /login" banner, mid-compaction, showing a permission
//! prompt, stuck with a half-finished bracketed-paste in the input,
//! and so on. Hand-written regex/state-machine detection lost to
//! this long tail (see how the bracketed-paste/submit split blocked
//! every Claude Code dispatch in the screenshots from 2026-05-11).
//!
//! The gate sidesteps the long tail by handing the pane contents to
//! a local LLM (gemma4 via the bridge) together with the project's
//! declared statuses — each carrying a free-form description of what
//! work in that column looks like. The model picks the status whose
//! description best matches what the pane is showing right now, and
//! surfaces a reason plus an optional `human_action` string when
//! human intervention is needed.
//!
//! ## Why this lives in the sipag binary, not sipag-core
//!
//! Originally `sipag-core::gate`. Moved here as part of §9 #9 (fold
//! gate into the lens-worker abstraction) — the gate now talks gemma
//! through `sipag_lens::ChatBackend` (concretely `BridgeChatBackend`)
//! instead of the legacy `sipag_core::llm::chat` direct-reqwest path.
//! ChatBackend lives in sipag-lens, sipag-lens isn't a sipag-core
//! dependency, so the cleanest placement is in the binary that owns
//! the bridge wiring.
//!
//! The output shape (`GateDecision { status_name, reason, human_action }`)
//! doesn't naturally map to one of the four lens-worker
//! `StructuralAction` verbs (the classification has no kr_ref, and
//! `human_action` has no slot in `SuggestStance`), so we keep the
//! gate's existing one-shot calling convention. The "fold" is about
//! the wire, not the call shape.
//!
//! See `sipag-board/src/project.rs` for the `Status` shape and its
//! `dispatchable` flag.

use serde::Deserialize;
use sipag_board::Status;
use sipag_lens::{ChatBackend, ChatOptions};

/// Inputs the gate needs to reason about a session.
pub struct GateInput<'a> {
    /// Task title — gives gemma context for what "ready" means.
    pub task_title: &'a str,
    /// Role command (e.g. `claude`, `claude --resume`) — tells gemma
    /// what kind of agent is supposed to receive the prompt.
    pub task_role: &'a str,
    /// Project statuses with descriptions. Gemma picks one of these
    /// by name; the descriptions tell it what each column means.
    pub statuses: &'a [Status],
    /// Last N lines of the visible pane (plain text from
    /// `KatulongClient::session_output_lines`). Caller chooses N —
    /// 40-80 is a sensible range; less than 20 starves the model of
    /// context, more than ~200 blows up the prompt without adding
    /// signal.
    pub session_output: &'a str,
}

/// Gate's verdict on a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateDecision {
    /// One of `statuses[i].name`. Coerced to `needs-human` when the
    /// model returns an unknown name or unparsable JSON.
    pub status_name: String,
    /// One short phrase explaining the choice. Persisted onto the
    /// task so the TUI / web UI can show it next to the column move.
    pub reason: String,
    /// What a human needs to do, if anything. `None` when the chosen
    /// status doesn't require human intervention (e.g. dispatchable
    /// or in-progress).
    pub human_action: Option<String>,
}

/// Status name the gate falls back to when the model misbehaves. The
/// project's default statuses include `needs-human`; if a custom
/// project omits it, the caller will see this name in the decision
/// and can decide whether to map it onto one of its own columns or
/// surface the error directly.
const FALLBACK_STATUS: &str = "needs-human";

/// Hard cap on how much raw model output we paste into a fallback
/// reason. Prevents a model that returns a multi-kilobyte rant from
/// bloating a task file.
const MAX_RAW_REASON_LEN: usize = 280;

/// Default model name the gate calls when `OLLAMA_MODEL` is unset.
/// Bridge-tier classifier — fast small model is the right pick.
/// Operators can override via `OLLAMA_MODEL` env var for parity with
/// the pre-fold gate's behavior (`sipag_core::llm::env_model`). Lift
/// to per-lens model selection via `~/.sipag/models.toml` if/when the
/// gate grows a `Lens` representation.
const DEFAULT_GATE_MODEL: &str = "gemma4:latest";

/// Resolve the gate's model: `OLLAMA_MODEL` env var if set,
/// otherwise [`DEFAULT_GATE_MODEL`]. Mirrors the pre-§9-#9
/// behavior of `sipag_core::llm::env_model`.
fn gate_model() -> String {
    std::env::var("OLLAMA_MODEL").unwrap_or_else(|_| DEFAULT_GATE_MODEL.to_string())
}

/// Sampling options the gate pins. Pre-§9-#9 the gate set these
/// explicitly in its `ChatOptions` so gemma's classification stayed
/// deterministic enough to parse reliably; we preserve them on the
/// new path via `ChatBackend::chat_with_options`.
const GATE_TEMPERATURE: f32 = 0.2;
const GATE_NUM_PREDICT: u32 = 512;

/// Classify the session and return the gate's verdict.
///
/// Calls gemma via the supplied `ChatBackend` — in production that's
/// `BridgeChatBackend`, which routes through `ollama-bridge-client`
/// (queue + auth + dedup) before reaching ollama. No retries — if
/// the bridge call itself fails (transport, timeout, HTTP error) this
/// propagates the error so the caller fails closed and skips
/// dispatch. If the model *replies* but the reply is unusable, the
/// function coerces to a `needs-human` decision rather than erroring,
/// so the operator always sees a row state they can react to.
pub async fn classify<B: ChatBackend>(
    backend: &B,
    input: GateInput<'_>,
) -> Result<GateDecision, GateError> {
    let user_prompt = render_user_prompt(&input);
    let model = gate_model();
    let options = ChatOptions {
        temperature: Some(GATE_TEMPERATURE),
        num_predict: Some(GATE_NUM_PREDICT),
    };
    let raw = backend
        .chat_with_options(&model, SYSTEM_PROMPT, &user_prompt, &options)
        .await
        .map_err(|e| GateError::Backend(e.to_string()))?;
    Ok(parse_decision(&raw, input.statuses))
}

/// What can go wrong calling the gate. Today only bridge transport
/// failures bubble — model-output parsing falls back to
/// `needs-human` inside [`parse_decision`] so the caller always
/// sees a decision.
#[derive(Debug)]
pub enum GateError {
    /// `ChatBackend::chat` failed (bridge unreachable, ollama down,
    /// timeout, auth, etc.). The error message is the formatted
    /// `LensError::Display` from sipag-lens.
    Backend(String),
}

impl std::fmt::Display for GateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GateError::Backend(msg) => write!(f, "gate bridge call failed: {msg}"),
        }
    }
}

impl std::error::Error for GateError {}

const SYSTEM_PROMPT: &str =
    "You are a dispatch gate for a remote interactive shell on a katulong host.\n\
    Sipag is about to type a task prompt into a tmux pane. Before it fires, your job is to look at \
    what is currently on screen in that pane and classify the session's state against a list of \
    statuses the operator has declared for this project.\n\
    \n\
    Each status has a name and a free-form description of what work in that column looks like. \
    Pick the one whose description best matches what you actually see in the pane. The pane may \
    show a logged-out screen, a permission prompt, a compaction in progress, a half-finished \
    bracketed-paste in the input box, a Claude TUI waiting at an idle prompt, a shell at PS1, an \
    error from a previous attempt, or anything else.\n\
    \n\
    Reply with a single JSON object on one line. No markdown, no commentary, no code fences.\n\
    Schema: {\"status\":\"<one of the listed status names>\",\"reason\":\"<one short phrase>\",\
    \"human_action\":\"<phrase or null>\"}\n\
    \n\
    `reason` is one phrase (≤ 140 chars) the operator will read next to the task on the board. \
    `human_action` is what a human should do to unblock the task, when one is needed. Set it to \
    null when no human action is required (e.g., the session is genuinely ready to receive work).";

fn render_user_prompt(input: &GateInput<'_>) -> String {
    let mut out = String::new();
    out.push_str("Task title: ");
    out.push_str(input.task_title);
    out.push_str("\nAgent role (will be launched as): ");
    out.push_str(input.task_role);
    out.push_str("\n\nProject statuses (pick exactly one `name`):\n");
    for s in input.statuses {
        out.push_str("- ");
        out.push_str(&s.name);
        if !s.description.is_empty() {
            out.push_str(": ");
            out.push_str(&s.description);
        }
        out.push('\n');
    }
    out.push_str("\nVisible pane right now:\n---\n");
    out.push_str(input.session_output);
    if !input.session_output.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("---\n");
    out
}

/// Parse the model's reply into a `GateDecision`. Forgiving by
/// design — extracts the JSON object even when the model wraps it in
/// prose, coerces unknown status names to `needs-human`, and falls
/// back to `needs-human` with the truncated raw output as the reason
/// when JSON parsing fails outright.
fn parse_decision(raw: &str, statuses: &[Status]) -> GateDecision {
    #[derive(Deserialize)]
    struct Reply {
        #[serde(default)]
        status: String,
        #[serde(default)]
        reason: String,
        #[serde(default)]
        human_action: Option<String>,
    }

    // Same {/} extraction the lens-worker's parse_tool_call uses —
    // gemma sometimes prefixes/suffixes the JSON with stray text
    // despite the schema line in the system prompt.
    let (start, end) = match (raw.find('{'), raw.rfind('}')) {
        (Some(s), Some(e)) if e > s => (s, e),
        _ => return fallback_unparsable(raw),
    };
    let json_str = &raw[start..=end];
    let parsed: Reply = match serde_json::from_str(json_str) {
        Ok(p) => p,
        Err(_) => return fallback_unparsable(raw),
    };

    let known = statuses.iter().any(|s| s.name == parsed.status);
    if !known {
        return GateDecision {
            status_name: FALLBACK_STATUS.to_string(),
            reason: format!(
                "model returned unknown status '{}' — coerced to {FALLBACK_STATUS}",
                parsed.status
            ),
            human_action: Some(
                "Check the session manually and either move the task to a valid column or update \
                 the project's statuses to include the one gemma4 wanted."
                    .into(),
            ),
        };
    }

    GateDecision {
        status_name: parsed.status,
        reason: parsed.reason,
        // Normalise empty strings / explicit "null" / whitespace to
        // `None` so the TUI can branch on `Option::is_some` instead
        // of also checking for empty.
        human_action: parsed.human_action.and_then(|s| {
            let trimmed = s.trim();
            if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("null") {
                None
            } else {
                Some(trimmed.to_string())
            }
        }),
    }
}

fn fallback_unparsable(raw: &str) -> GateDecision {
    let mut snippet = raw.trim().to_string();
    if snippet.len() > MAX_RAW_REASON_LEN {
        snippet.truncate(MAX_RAW_REASON_LEN);
        snippet.push('…');
    }
    GateDecision {
        status_name: FALLBACK_STATUS.to_string(),
        reason: format!("gemma4 reply was not parseable JSON; raw: {snippet}"),
        human_action: Some(
            "Inspect the session manually — gemma4 returned non-JSON output instead of a \
             classification."
                .into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use sipag_lens::{ChatBackend, LensError, LensResult};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    fn sample_statuses() -> Vec<Status> {
        vec![
            Status {
                name: "todo".into(),
                description: "session is logged in, idle, and ready to dispatch".into(),
                dispatchable: true,
            },
            Status {
                name: "needs-human".into(),
                description: "human intervention required (login, permission, etc.)".into(),
                dispatchable: false,
            },
            Status {
                name: "in-progress".into(),
                description: "agent is actively working".into(),
                dispatchable: false,
            },
        ]
    }

    // ── canned chat backend for tests ──────────────────────────────

    struct CannedBackend {
        reply: String,
        captured_model: Arc<Mutex<Option<String>>>,
        captured_system: Arc<Mutex<Option<String>>>,
        captured_user: Arc<Mutex<Option<String>>>,
        captured_options: Arc<Mutex<Option<ChatOptions>>>,
    }

    impl CannedBackend {
        fn new(reply: impl Into<String>) -> Self {
            Self {
                reply: reply.into(),
                captured_model: Arc::new(Mutex::new(None)),
                captured_system: Arc::new(Mutex::new(None)),
                captured_user: Arc::new(Mutex::new(None)),
                captured_options: Arc::new(Mutex::new(None)),
            }
        }
    }

    #[async_trait]
    impl ChatBackend for CannedBackend {
        async fn chat(&self, model: &str, system: &str, user: &str) -> LensResult<String> {
            *self.captured_model.lock().await = Some(model.to_string());
            *self.captured_system.lock().await = Some(system.to_string());
            *self.captured_user.lock().await = Some(user.to_string());
            Ok(self.reply.clone())
        }

        async fn chat_with_options(
            &self,
            model: &str,
            system: &str,
            user: &str,
            options: &ChatOptions,
        ) -> LensResult<String> {
            *self.captured_model.lock().await = Some(model.to_string());
            *self.captured_system.lock().await = Some(system.to_string());
            *self.captured_user.lock().await = Some(user.to_string());
            *self.captured_options.lock().await = Some(options.clone());
            Ok(self.reply.clone())
        }
    }

    struct FailingBackend;

    #[async_trait]
    impl ChatBackend for FailingBackend {
        async fn chat(&self, _: &str, _: &str, _: &str) -> LensResult<String> {
            Err(LensError::Bridge("simulated transport failure".into()))
        }
    }

    // ── feature-requirement tests ──────────────────────────────────
    //
    // Tests pin what the gate PROMISES to its callers:
    // - classify routes through the supplied ChatBackend with the
    //   gate-tier model + the documented system prompt;
    // - successful parse produces the right GateDecision shape;
    // - bridge transport failures propagate as GateError::Backend;
    // - unparsable / unknown-status replies coerce to needs-human
    //   inside classify (caller always sees a decision, never a
    //   parse error);
    // - the prompt renderer includes all four input fields verbatim.

    #[test]
    fn rendered_prompt_includes_all_inputs() {
        let statuses = sample_statuses();
        let prompt = render_user_prompt(&GateInput {
            task_title: "Patch the handler",
            task_role: "claude",
            statuses: &statuses,
            session_output: "$ ls\nREADME.md\n$ ",
        });
        // All four sections must appear so the model has the inputs
        // it needs to classify. These asserts double as a snapshot
        // contract — if any disappears, every prompt-shape change
        // will fail visibly.
        assert!(prompt.contains("Patch the handler"));
        assert!(prompt.contains("Agent role (will be launched as): claude"));
        assert!(prompt.contains("- todo: session is logged in"));
        assert!(prompt.contains("- needs-human:"));
        assert!(prompt.contains("---\n$ ls\nREADME.md\n$ \n---"));
    }

    #[test]
    fn parse_decision_happy_path() {
        let raw = r#"{"status":"todo","reason":"idle shell","human_action":null}"#;
        let d = parse_decision(raw, &sample_statuses());
        assert_eq!(d.status_name, "todo");
        assert_eq!(d.reason, "idle shell");
        assert_eq!(d.human_action, None);
    }

    #[test]
    fn parse_decision_extracts_json_wrapped_in_prose() {
        let raw =
            "Sure! Here's my answer: {\"status\":\"todo\",\"reason\":\"ready\",\"human_action\":null} Hope that helps.";
        let d = parse_decision(raw, &sample_statuses());
        assert_eq!(d.status_name, "todo");
        assert_eq!(d.reason, "ready");
    }

    #[test]
    fn parse_decision_unparsable_falls_back_to_needs_human() {
        let raw = "I don't know how to answer that.";
        let d = parse_decision(raw, &sample_statuses());
        assert_eq!(d.status_name, FALLBACK_STATUS);
        assert!(d.reason.contains("not parseable"));
        assert!(d.human_action.is_some());
    }

    #[test]
    fn parse_decision_unknown_status_coerced_to_needs_human() {
        let raw = r#"{"status":"in-flight","reason":"agent thinking","human_action":null}"#;
        let d = parse_decision(raw, &sample_statuses());
        assert_eq!(d.status_name, FALLBACK_STATUS);
        assert!(d.reason.contains("in-flight"));
        assert!(d.reason.contains("coerced"));
        assert!(d.human_action.is_some());
    }

    #[test]
    fn parse_decision_normalizes_human_action_empty_and_null_strings() {
        let a = parse_decision(
            r#"{"status":"todo","reason":"r","human_action":"null"}"#,
            &sample_statuses(),
        );
        assert_eq!(a.human_action, None);

        let b = parse_decision(
            r#"{"status":"todo","reason":"r","human_action":"  "}"#,
            &sample_statuses(),
        );
        assert_eq!(b.human_action, None);

        let c = parse_decision(
            r#"{"status":"todo","reason":"r","human_action":"press 1 to approve"}"#,
            &sample_statuses(),
        );
        assert_eq!(c.human_action.as_deref(), Some("press 1 to approve"));
    }

    #[test]
    fn parse_decision_truncates_long_raw_in_fallback_reason() {
        let long = "x".repeat(MAX_RAW_REASON_LEN * 4);
        let d = parse_decision(&long, &sample_statuses());
        assert_eq!(d.status_name, FALLBACK_STATUS);
        assert!(d.reason.len() < long.len());
        assert!(d.reason.ends_with("…"));
    }

    // ── classify() integration with ChatBackend ────────────────────

    #[tokio::test]
    async fn classify_routes_through_chat_backend_with_gate_model_and_system_prompt() {
        // Pins the fold contract: classify() goes through ChatBackend,
        // not the legacy llm::chat path. The model name passed is the
        // gate-tier choice; the system prompt is the documented one.
        let backend =
            CannedBackend::new(r#"{"status":"todo","reason":"ready","human_action":null}"#);
        let statuses = sample_statuses();
        let decision = classify(
            &backend,
            GateInput {
                task_title: "T",
                task_role: "claude",
                statuses: &statuses,
                session_output: "$ ",
            },
        )
        .await
        .unwrap();
        assert_eq!(decision.status_name, "todo");
        let captured_model = backend.captured_model.lock().await.clone().unwrap();
        let captured_system = backend.captured_system.lock().await.clone().unwrap();
        assert_eq!(
            captured_model, DEFAULT_GATE_MODEL,
            "classify must call ChatBackend with the gate-tier model (default when OLLAMA_MODEL unset)"
        );
        assert_eq!(
            captured_system, SYSTEM_PROMPT,
            "classify must use the documented dispatch-gate system prompt verbatim"
        );
    }

    #[tokio::test]
    async fn classify_pins_gate_sampling_options_via_chat_with_options() {
        // Pre-§9-#9 the gate set temperature=0.2 + num_predict=512
        // explicitly. The fold must preserve this — without it ollama
        // defaults to ~0.8 temperature + unbounded num_predict, which
        // causes JSON-shape drift and more `fallback_unparsable`
        // coercions → more spurious needs-human parks of actually
        // dispatchable sessions.
        let backend = CannedBackend::new(r#"{"status":"todo","reason":"r","human_action":null}"#);
        let statuses = sample_statuses();
        classify(
            &backend,
            GateInput {
                task_title: "T",
                task_role: "claude",
                statuses: &statuses,
                session_output: "$ ",
            },
        )
        .await
        .unwrap();
        let options = backend.captured_options.lock().await.clone();
        let opts = options.expect("classify must use chat_with_options, not bare chat");
        assert_eq!(
            opts.temperature,
            Some(GATE_TEMPERATURE),
            "gate must pin temperature for deterministic structured output"
        );
        assert_eq!(
            opts.num_predict,
            Some(GATE_NUM_PREDICT),
            "gate must pin num_predict to bound model output length"
        );
    }

    // Note: OLLAMA_MODEL env-override behavior is exercised by
    // operators directly. We don't unit-test it here because
    // `std::env::set_var` is process-global and test parallelism
    // would interleave with any other test reading `OLLAMA_MODEL`.
    // The default-when-unset path IS covered by
    // `classify_routes_through_chat_backend_with_gate_model_and_system_prompt`
    // (the test runs without OLLAMA_MODEL set and asserts DEFAULT_GATE_MODEL).

    #[tokio::test]
    async fn classify_user_turn_contains_rendered_prompt() {
        let backend = CannedBackend::new(r#"{"status":"todo","reason":"r","human_action":null}"#);
        let statuses = sample_statuses();
        classify(
            &backend,
            GateInput {
                task_title: "Patch the handler",
                task_role: "claude",
                statuses: &statuses,
                session_output: "$ ls\n",
            },
        )
        .await
        .unwrap();
        let captured = backend.captured_user.lock().await.clone().unwrap();
        assert!(captured.contains("Patch the handler"));
        assert!(captured.contains("- todo:"));
        assert!(captured.contains("$ ls"));
    }

    #[tokio::test]
    async fn classify_propagates_bridge_transport_failures() {
        let backend = FailingBackend;
        let statuses = sample_statuses();
        let err = classify(
            &backend,
            GateInput {
                task_title: "T",
                task_role: "claude",
                statuses: &statuses,
                session_output: "$ ",
            },
        )
        .await
        .unwrap_err();
        match err {
            GateError::Backend(msg) => {
                assert!(
                    msg.contains("simulated"),
                    "must surface the underlying transport error message"
                );
            }
        }
    }

    #[tokio::test]
    async fn classify_coerces_unparseable_to_needs_human_without_erroring() {
        // If the bridge succeeds but the model replies with garbage,
        // classify returns a needs-human decision rather than an
        // error. Caller always sees a usable row state.
        let backend = CannedBackend::new("definitely not json");
        let statuses = sample_statuses();
        let decision = classify(
            &backend,
            GateInput {
                task_title: "T",
                task_role: "claude",
                statuses: &statuses,
                session_output: "$ ",
            },
        )
        .await
        .unwrap();
        assert_eq!(decision.status_name, FALLBACK_STATUS);
        assert!(decision.human_action.is_some());
    }
}
