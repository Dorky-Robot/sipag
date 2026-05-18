//! Dispatch nudge loop — gemma4 observes the pane and reports state.
//!
//! ## Status (2026-05-17)
//!
//! **Early lens-worker prototype.** This module is an early version
//! of the lens-worker pattern documented in `docs/modules.md` §3
//! (Experimentation): gemma reads context, returns a structured
//! classification, sipag dispatches on it. When Phase 1 #3 lands
//! (modules.md §9), this folds into the lens-worker abstraction as
//! a worker with a "post-dispatch progress" lens.
//!
//! Until then: the legacy keystroke-driving path in
//! `sipag/src/serve/htmx.rs::verify_and_heal_dispatch` still
//! uses this module to drive paste / submit / recover. That entire
//! path retires when Phase 2 #11 in modules.md §9 lands (attach
//! client owns keystrokes; recovery loop is deleted, not refactored).
//! `NudgeDecision::keystrokes` becomes informational only after
//! that — the mechanical paste/submit handshake is owned by the
//! attach client, not gemma. See memory `feedback-strict-layer-coupling`
//! for the rationale (the bridge is gemma's only job; mechanical
//! actuation is a different concern).
//!
//! ## Original (transitional) design
//!
//! The pre-dispatch [`crate::gate`] decides *whether* to fire. After
//! it does, the pane still has to get from "claude TUI just launched"
//! to "agent is actively working on the task." That transition used
//! to be a hand-rolled bracketed-paste + verify + propose-recovery
//! sequence in `sipag/src/serve/htmx.rs` — three separate state
//! machines each with their own quirks (the trailing `\r` getting
//! absorbed into the paste, `agent.running` reporting true on an idle
//! Claude TUI, gemma's recovery proposing a second paste on top of
//! the stuck first one).
//!
//! The nudge loop collapses all three into one LLM judgment. Every
//! tick: read pane → ask gemma `{ status, keystrokes, done, reason,
//! human_action }` → send the keystrokes → repeat. Gemma reasons
//! about the full picture (paste already in the input? still need to
//! press Enter? trust prompt blocking? login required? agent now
//! processing?) and emits one coherent decision per tick. The caller
//! stops when `done = true` or after a hard iteration cap.
//!
//! Distinct from [`crate::gate`] only in the schema: a `GateDecision`
//! says "what column does this session belong in"; a `NudgeDecision`
//! says that AND "what should I type next." The two share the same
//! local-ollama transport via [`crate::llm`].

use serde::Deserialize;

use crate::board::Status;
use crate::llm::{self, ChatMessage, ChatOptions, LlmError};

/// Inputs for one nudge-loop tick.
pub struct NudgeInput<'a> {
    pub task_title: &'a str,
    /// What the agent command line will look like (e.g. `claude`,
    /// `claude --resume`). Tells gemma what's in the pane after the
    /// launch exec.
    pub task_role_cmd: &'a str,
    /// The full templated dispatch prompt (Context / Research first /
    /// Task). Gemma uses this both to know what to type into the
    /// input box and to recognise when the same text is already
    /// visible there.
    pub intended_prompt: &'a str,
    /// Project's declared statuses with descriptions.
    pub statuses: &'a [Status],
    /// Last N lines of the visible pane (plain text via
    /// `KatulongClient::session_output_lines`).
    pub session_output: &'a str,
    /// 1-based current iteration. Gemma sees this so it can choose
    /// less aggressive steps early and more decisive ones late.
    pub iteration: u8,
    /// Hard cap configured by the caller. Gemma sees the budget so it
    /// can prefer to mark `done=true` rather than nudge forever.
    pub max_iterations: u8,
}

/// One tick's verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NudgeDecision {
    /// Which project status the session currently best matches.
    /// Persisted onto the task so the board reflects state in real
    /// time during the loop.
    pub status_name: String,
    /// Short phrase explaining the choice; shown on the task row.
    pub reason: String,
    /// What a human needs to do, when human action is required.
    pub human_action: Option<String>,
    /// Raw bytes to send to the pane this tick via katulong's
    /// `exec_session`. `None` means "wait — observe again next tick."
    /// Supports `\n` for Enter, `\u{0003}` for Ctrl-C, `\u{0004}` for
    /// Ctrl-D, and bracketed-paste sequences (`\x1b[200~…\x1b[201~`)
    /// for multi-line text. Gemma should send the submit `\n`
    /// **separately** from the paste — the closing BPM marker
    /// followed immediately by `\r` in one keystroke is the bug that
    /// made dispatches stick.
    pub keystrokes: Option<String>,
    /// Terminal signal. `true` means the loop should stop:
    /// - status is `in-progress` and agent is observably processing,
    /// - or a `needs-human` blocker requires intervention,
    /// - or the work is otherwise concluded.
    pub done: bool,
}

const FALLBACK_STATUS: &str = "needs-human";
const MAX_RAW_REASON_LEN: usize = 280;

/// Ask gemma for the next step.
///
/// Mirrors [`crate::gate::classify`]'s contract: transport errors are
/// returned as `Err(LlmError)` so the caller can fail closed; bad
/// model replies fall back to a `needs-human` terminal decision so
/// the operator always sees a row state they can react to.
pub async fn next_step(
    http: &reqwest::Client,
    input: NudgeInput<'_>,
) -> std::result::Result<NudgeDecision, LlmError> {
    let prompt = render_user_prompt(&input);
    let opts = ChatOptions {
        temperature: 0.2,
        // Slightly larger than gate's 512 — nudge replies include
        // keystrokes which can carry the full prompt body inside a
        // bracketed-paste literal.
        num_predict: Some(2048),
        ..Default::default()
    };
    let messages = vec![
        ChatMessage::system(SYSTEM_PROMPT),
        ChatMessage::user(prompt),
    ];
    let raw = llm::chat(http, &llm::env_host(), messages, opts).await?;
    Ok(parse_decision(&raw, input.statuses))
}

const SYSTEM_PROMPT: &str = "You are a self-driving dispatch operator for a Claude Code session running in a tmux pane on a katulong host.\n\
    \n\
    Sipag wants a task running in this pane. The launch command for the agent has already been typed. \
    Your job, one tick at a time, is to read what is on screen and decide what to do next: send some \
    keystrokes to advance the session, or just observe again, or stop because the work is in flight \
    or a human must intervene.\n\
    \n\
    At each tick, return a single JSON object on one line. No markdown, no commentary, no code fences.\n\
    Schema: {\"status\":\"<one of the listed status names>\",\"reason\":\"<one short phrase>\",\
    \"human_action\":\"<phrase or null>\",\"keystrokes\":\"<raw bytes or null>\",\"done\":<true or false>}\n\
    \n\
    `status` and `reason` mirror the dispatch gate's schema — pick the project status whose description \
    best matches what you see in the pane. `human_action` is what a human must do to unblock the task \
    when one is needed; null when no human action is required.\n\
    \n\
    `keystrokes` is the raw bytes you want to type into the pane this tick:\n\
    - `\\n` for Enter (submit a line)\n\
    - `\\u0003` for Ctrl-C, `\\u0004` for Ctrl-D\n\
    - For multi-line text, wrap in bracketed-paste markers: `\\u001b[200~<text>\\u001b[201~` (use JSON `\\u001b` for ESC, NOT `\\x1b` — JSON doesn't recognise `\\x`)\n\
    - IMPORTANT: Claude Code's input box treats a paste as one chunk. The submit Enter must be sent in a \
    SEPARATE tick (one tick sends the paste; the next tick sends just `\\n`). If you put the `\\n` inside \
    the same keystrokes string as the closing BPM marker, the Enter gets absorbed into the paste and the \
    prompt never submits — this is the failure mode you exist to prevent.\n\
    - null when no keys should be sent this tick (just observe again).\n\
    \n\
    `done` is the terminal signal. Set it true ONLY when:\n\
    - the agent is observably processing the task (status is the dispatchable column's counterpart, \
    typically `in-progress`), OR\n\
    - a `needs-human` blocker requires intervention before anything else can happen, OR\n\
    - the work is otherwise concluded.\n\
    \n\
    The user message will give you the task, the intended prompt to feed the agent, the project's \
    status options, the current pane contents, and your iteration budget. Prefer small steps early; \
    if you near the budget without progress, mark `done=true` with `needs-human` rather than nudging \
    in circles.";

fn render_user_prompt(input: &NudgeInput<'_>) -> String {
    let mut out = String::new();
    out.push_str("Task title: ");
    out.push_str(input.task_title);
    out.push_str("\nAgent role command: ");
    out.push_str(input.task_role_cmd);
    out.push_str(&format!(
        "\nIteration {} of {} (budget remaining: {})\n",
        input.iteration,
        input.max_iterations,
        input.max_iterations.saturating_sub(input.iteration)
    ));

    out.push_str(
        "\nIntended prompt to feed the agent (paste this into Claude's input then submit):\n---\n",
    );
    out.push_str(input.intended_prompt);
    if !input.intended_prompt.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("---\n\n");

    out.push_str("Project statuses (pick exactly one `name`):\n");
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

fn parse_decision(raw: &str, statuses: &[Status]) -> NudgeDecision {
    #[derive(Deserialize)]
    struct Reply {
        #[serde(default)]
        status: String,
        #[serde(default)]
        reason: String,
        #[serde(default)]
        human_action: Option<String>,
        #[serde(default)]
        keystrokes: Option<String>,
        #[serde(default)]
        done: bool,
    }

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
        return NudgeDecision {
            status_name: FALLBACK_STATUS.to_string(),
            reason: format!(
                "model returned unknown status '{}' — coerced to {FALLBACK_STATUS}",
                parsed.status
            ),
            human_action: Some(
                "Check the session manually and either move the task to a valid column or update \
                 the project's statuses."
                    .into(),
            ),
            keystrokes: None,
            done: true,
        };
    }

    NudgeDecision {
        status_name: parsed.status,
        reason: parsed.reason,
        human_action: normalize_optional_string(parsed.human_action),
        keystrokes: normalize_optional_string(parsed.keystrokes),
        done: parsed.done,
    }
}

fn normalize_optional_string(s: Option<String>) -> Option<String> {
    s.and_then(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("null") {
            None
        } else {
            // Preserve internal whitespace (keystrokes may legitimately
            // include leading/trailing spaces inside a paste body); only
            // trim was done to detect the empty-string case.
            Some(raw)
        }
    })
}

fn fallback_unparsable(raw: &str) -> NudgeDecision {
    let mut snippet = raw.trim().to_string();
    if snippet.len() > MAX_RAW_REASON_LEN {
        snippet.truncate(MAX_RAW_REASON_LEN);
        snippet.push('…');
    }
    NudgeDecision {
        status_name: FALLBACK_STATUS.to_string(),
        reason: format!("gemma4 reply was not parseable JSON; raw: {snippet}"),
        human_action: Some(
            "Inspect the session manually — gemma4 returned non-JSON output mid-nudge-loop.".into(),
        ),
        keystrokes: None,
        done: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_statuses() -> Vec<Status> {
        vec![
            Status {
                name: "todo".into(),
                description: "ready to dispatch — idle prompt".into(),
                dispatchable: true,
            },
            Status {
                name: "in-progress".into(),
                description: "agent actively processing the task".into(),
                dispatchable: false,
            },
            Status {
                name: "needs-human".into(),
                description: "human must intervene (login, permission, paste-stuck, etc.)".into(),
                dispatchable: false,
            },
        ]
    }

    fn sample_input<'a>(statuses: &'a [Status], pane: &'a str) -> NudgeInput<'a> {
        NudgeInput {
            task_title: "Fix the thing",
            task_role_cmd: "claude",
            intended_prompt: "## Context\nDo the work.\n",
            statuses,
            session_output: pane,
            iteration: 1,
            max_iterations: 20,
        }
    }

    #[test]
    fn rendered_prompt_includes_task_intended_prompt_statuses_pane_and_budget() {
        let statuses = sample_statuses();
        let prompt = render_user_prompt(&sample_input(&statuses, "$ "));
        assert!(prompt.contains("Fix the thing"));
        assert!(prompt.contains("Iteration 1 of 20"));
        assert!(prompt.contains("budget remaining: 19"));
        assert!(prompt.contains("## Context\nDo the work."));
        assert!(prompt.contains("- todo: ready to dispatch"));
        assert!(prompt.contains("- in-progress: agent actively processing"));
        // Pane is delimited so gemma can locate its boundaries.
        assert!(prompt.contains("Visible pane right now:\n---\n$ \n---"));
    }

    #[test]
    fn parse_decision_happy_path_with_keystrokes_and_not_done() {
        // Iteration 1: claude TUI just launched, pane shows empty
        // prompt; gemma proposes the bracketed paste, marks not done.
        let raw = r#"{"status":"todo","reason":"claude TUI ready; pasting prompt","human_action":null,"keystrokes":"\u001b[200~hello\u001b[201~","done":false}"#;
        let d = parse_decision(raw, &sample_statuses());
        assert_eq!(d.status_name, "todo");
        assert_eq!(d.reason, "claude TUI ready; pasting prompt");
        assert_eq!(d.human_action, None);
        assert_eq!(
            d.keystrokes.as_deref(),
            Some("\u{001b}[200~hello\u{001b}[201~")
        );
        assert!(!d.done);
    }

    #[test]
    fn parse_decision_terminal_done() {
        let raw = r#"{"status":"in-progress","reason":"agent is reading files","human_action":null,"keystrokes":null,"done":true}"#;
        let d = parse_decision(raw, &sample_statuses());
        assert_eq!(d.status_name, "in-progress");
        assert!(d.done);
        assert_eq!(d.keystrokes, None);
    }

    #[test]
    fn parse_decision_extracts_json_wrapped_in_prose() {
        let raw = "Here you go: {\"status\":\"todo\",\"reason\":\"ready\",\"human_action\":null,\"keystrokes\":null,\"done\":false} — that's my next step.";
        let d = parse_decision(raw, &sample_statuses());
        assert_eq!(d.status_name, "todo");
    }

    #[test]
    fn parse_decision_unparsable_terminal_needs_human() {
        let raw = "I don't know how to answer that.";
        let d = parse_decision(raw, &sample_statuses());
        assert_eq!(d.status_name, FALLBACK_STATUS);
        assert!(d.done);
        assert!(d.reason.contains("not parseable"));
    }

    #[test]
    fn parse_decision_unknown_status_terminal_needs_human() {
        let raw = r#"{"status":"in-flight","reason":"thinking","human_action":null,"keystrokes":null,"done":false}"#;
        let d = parse_decision(raw, &sample_statuses());
        assert_eq!(d.status_name, FALLBACK_STATUS);
        // Unknown status is treated as terminal so the loop stops and
        // the operator sees a row to act on, rather than nudging into
        // a fictional column forever.
        assert!(d.done);
        assert!(d.reason.contains("in-flight"));
    }

    #[test]
    fn parse_decision_normalizes_null_strings_to_none() {
        let raw =
            r#"{"status":"todo","reason":"r","human_action":"null","keystrokes":"","done":false}"#;
        let d = parse_decision(raw, &sample_statuses());
        assert_eq!(d.human_action, None);
        assert_eq!(d.keystrokes, None);
    }

    #[test]
    fn parse_decision_preserves_keystrokes_internal_whitespace() {
        // A bracketed paste with leading/trailing spaces inside the
        // body must round-trip exactly. We only trim for empty-string
        // detection.
        let raw = r#"{"status":"todo","reason":"r","human_action":null,"keystrokes":"\u001b[200~  spaces  \u001b[201~","done":false}"#;
        let d = parse_decision(raw, &sample_statuses());
        assert_eq!(
            d.keystrokes.as_deref(),
            Some("\u{001b}[200~  spaces  \u{001b}[201~")
        );
    }
}
