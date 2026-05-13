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
//! a local LLM (gemma4 via ollama on `OLLAMA_HOST`) together with the
//! project's declared statuses — each carrying a free-form
//! description of what work in that column looks like. The model
//! picks the status whose description best matches what the pane is
//! showing right now, and surfaces a reason plus an optional
//! `human_action` string when human intervention is needed.
//!
//! See `sipag-core/src/board/project.rs` for the `Status` shape and
//! its `dispatchable` flag. The dispatch wiring in
//! `sipag/src/cli.rs::run_dispatch_task` consults the gate before
//! `exec_session(agent_cmd)` and refuses to fire when the chosen
//! status is anything other than the project's dispatchable one.

use serde::Deserialize;

use crate::board::Status;
use crate::llm::{self, ChatMessage, ChatOptions, LlmError};

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

/// Classify the session and return the gate's verdict.
///
/// Calls gemma4 (or whatever model `OLLAMA_MODEL` resolves to) via
/// the existing `llm::chat` SSE client. No retries — if the model
/// call itself fails (transport, timeout, HTTP error) this propagates
/// the error so the caller fails closed and skips dispatch. If the
/// model *replies* but the reply is unusable, the function coerces
/// to a `needs-human` decision rather than erroring, so the operator
/// always sees a row state they can react to.
pub async fn classify(
    http: &reqwest::Client,
    input: GateInput<'_>,
) -> std::result::Result<GateDecision, LlmError> {
    let prompt = render_user_prompt(&input);
    let opts = ChatOptions {
        temperature: 0.2,
        num_predict: Some(512),
        ..Default::default()
    };
    let messages = vec![
        ChatMessage::system(SYSTEM_PROMPT),
        ChatMessage::user(prompt),
    ];
    let raw = llm::chat(http, &llm::env_host(), messages, opts).await?;
    Ok(parse_decision(&raw, input.statuses))
}

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

    // Same {/} extraction `propose_recovery` uses — gemma sometimes
    // prefixes/suffixes the JSON with stray text despite the schema
    // line in the system prompt.
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
        // Gemma occasionally prefixes the JSON with an explanation
        // despite the schema line. The {/} extractor recovers.
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
        // Model picked a status not on the project's list — coerce
        // rather than blindly trusting the name, so the dispatcher
        // never tries to move a task to a column that doesn't exist.
        let raw = r#"{"status":"in-flight","reason":"agent thinking","human_action":null}"#;
        let d = parse_decision(raw, &sample_statuses());
        assert_eq!(d.status_name, FALLBACK_STATUS);
        assert!(d.reason.contains("in-flight"));
        assert!(d.reason.contains("coerced"));
        assert!(d.human_action.is_some());
    }

    #[test]
    fn parse_decision_normalizes_human_action_empty_and_null_strings() {
        // `"null"` and `""` from the model both become `None`. The
        // TUI branches on `is_some`, so we need a clean signal.
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
        // A model that returns a wall of prose shouldn't bloat the
        // task file. Reason text is bounded.
        let long = "x".repeat(MAX_RAW_REASON_LEN * 4);
        let d = parse_decision(&long, &sample_statuses());
        assert_eq!(d.status_name, FALLBACK_STATUS);
        assert!(d.reason.len() < long.len());
        assert!(d.reason.ends_with("…"));
    }
}
