//! sipag-dispatch — the **act** sub-module of the Experimentation
//! context (per `docs/extraction-plan.md` and `docs/modules.md` §9
//! Phase 2 #12).
//!
//! Encapsulates the single dispatch action: take a katulong host
//! config + a typed input, optionally set up a worktree, attach over
//! WebSocket, launch the agent, paste the prompt, submit, and wait
//! for the agent to start processing. One function, one flow,
//! observable via a step callback so callers (CLI + web UI) can
//! report progress in their own idiom.
//!
//! ## Design constraints
//!
//! - **Loaders live outside.** Callers pre-load `Task` / `Role` /
//!   `Project` / `Objective` / `KeyResult` from `sipag-core::board::*`
//!   and pass the resolved fields in as [`DispatchInput`]. Keeps
//!   this crate's dependency graph tight (just `katulong-client`).
//! - **Prompt composition lives outside.** The prompt body is a
//!   field on [`DispatchInput`]; this crate doesn't know about
//!   Objectives or KRs. The caller composes the prompt with whatever
//!   Steering-context knowledge it wants.
//! - **Outcome publishing lives outside.** The [`dispatch`] function
//!   takes an `on_step` callback. CLI callers can `println!`; web UI
//!   callers can publish to the [`sipag-pubsub`] broker.
//! - **The wire path is unified.** Both the CLI and the web UI go
//!   through this function. Previously the CLI used HTTP `/exec` for
//!   the agent launch (no orchestration); now both paths use the
//!   `KatulongAttachClient`'s `wait_for` orchestration (TUI-ready
//!   wait, paste echo, processing wait).
//!
//! ## What this crate is NOT
//!
//! - **Not the gate.** Pre-flight classification (gemma4 deciding
//!   whether to dispatch) is a separate concern and retires
//!   separately (the gate's load-bearing use case dissolved with the
//!   per-dispatch session model). See `docs/modules.md` §9 #11.
//! - **Not the nudge loop.** Post-dispatch observation belongs to
//!   the lens-worker abstraction (see `docs/modules.md` §9 #3). The
//!   legacy keystroke-driving recovery loop (`verify_and_heal_dispatch`)
//!   was deleted entirely with §9 #11 (closes sipag #528 by deletion
//!   — see also memory `feedback-strict-layer-coupling`).
//! - **Not session lifecycle policy.** This function expects the
//!   caller to have created the katulong session already (so the
//!   gate could inspect it). On dispatch failure the session is
//!   left in place — the caller decides whether to kill it, retry,
//!   or hand it off to an operator.

use katulong_client::{
    attach::{DEFAULT_ATTACH_COLS, DEFAULT_ATTACH_ROWS},
    AttachError, KatulongAsyncClient, KatulongAsyncError, KatulongAttach, KatulongAttachClient,
    KeyName, RemoteConfig, TmuxSession, WaitFrom,
};

// Note: anyhow is a transitive concern surfaced through
// `KatulongAsyncClient::new` (which uses `anyhow::Result` for
// reqwest builder construction). We import it via its return type
// rather than as a public dep on this crate's surface.
use std::time::Duration;
use thiserror::Error;
use tracing::{info, warn};

/// Wait for the agent's TUI to render after the launch keystroke.
/// 30s accommodates cold starts where MCP servers / auth checks
/// delay the first frame.
const TUI_READY_TIMEOUT: Duration = Duration::from_secs(30);

/// Best-effort wait for the prompt body to echo into the agent's
/// input box. A missed echo is logged and dispatch proceeds — the
/// echo wait is observation, not a gate.
const PASTE_ECHO_TIMEOUT: Duration = Duration::from_secs(3);

/// Wait for the agent to start processing after submit. 10s covers
/// the agent's first-tool / first-response render.
const PROCESSING_TIMEOUT: Duration = Duration::from_secs(10);

/// Inputs to one dispatch action. Caller resolves task / role /
/// prompt out-of-band and passes only the fields this crate needs.
#[derive(Debug, Clone)]
pub struct DispatchInput {
    /// Stable task id. Used only for tracing / log lines; no wire
    /// effect.
    pub task_id: u64,
    /// Project namespace. Tracing only.
    pub project_name: String,
    /// The unique-per-dispatch portion of the prompt — used to
    /// compile the paste-echo regex. Typically the task title.
    pub task_title: String,
    /// Command to launch the agent inside the katulong PTY,
    /// e.g. `"claude --dangerously-skip-permissions"`.
    pub role_command: String,
    /// Full prompt body. Pasted into the agent's input box once
    /// its TUI reports ready.
    pub prompt: String,
    /// Optional worktree setup. When `Some`, the contained shell
    /// command runs via HTTP `/exec` *before* the WS attach is
    /// opened. When `None`, the launch runs in the session's
    /// default cwd.
    pub worktree: Option<WorktreeSpec>,
}

/// Spec for the optional worktree setup step. Pulled into its own
/// type so callers can construct it from
/// `katulong_client::worktree_command(project, task_id)` +
/// `katulong_client::worktree_path(project, task_id)` or a custom
/// shell snippet + path pair of their choice.
#[derive(Debug, Clone)]
pub struct WorktreeSpec {
    /// Shell command run via HTTP `/exec`. Example:
    /// `"cd /work/<project> && git worktree add .worktrees/task-<id> -b fix/task-<id>"`.
    /// The command is run inside the katulong-managed shell, so it
    /// inherits whatever cwd / env the session started with.
    pub setup_command: String,
    /// Working directory the agent should run in. Used to prepend
    /// `cd <path> && ` to the launch keystroke so the agent starts
    /// inside the freshly-created worktree, not in whatever cwd the
    /// shell happened to land in after `setup_command` returned.
    /// Typically `katulong_client::worktree_path(project, task_id)`.
    ///
    /// **Input contract:** `path` is interpolated into a POSIX shell
    /// command (`cd <path> && ...`) without quoting. The caller is
    /// responsible for passing a shell-safe path — same contract as
    /// `setup_command`. `katulong_client::worktree_path` produces
    /// `/work/<project>/.worktrees/task-<id>`, all alphanumeric +
    /// hyphens + slashes, no quoting needed. A path containing
    /// spaces, single quotes, or shell metacharacters would silently
    /// break the launch.
    ///
    /// (Pre-extraction, the agent's launch command was a one-shot
    /// `cd <path> && <role_command> -p '<prompt>'` via HTTP `/exec`;
    /// the new flow runs the setup over HTTP and launches the agent
    /// over WS attach, so the cd has to be threaded through
    /// explicitly. Forgetting it lets the agent run at the project
    /// root, which silently breaks worktree isolation.)
    pub path: String,
}

/// One observable step in the dispatch flow. Surfaced to the caller
/// via the `on_step` callback. The callback fires *before* each
/// step's work runs — useful for "spinner" UIs and broker events.
///
/// Steps fire in source-order; a failure short-circuits the rest.
/// [`WaitEcho`](DispatchStep::WaitEcho) is best-effort — its
/// failure is logged but does not abort the flow.
///
/// Session creation is the caller's responsibility — the gate +
/// state-pinning logic in the web UI needs to run after the session
/// exists but before dispatch. [`dispatch`] takes the pre-created
/// session as input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchStep {
    /// `POST /sessions/by-id/<id>/exec` with the worktree setup
    /// command. Only fires when `input.worktree.is_some()`.
    WorktreeSetup,
    /// WebSocket attach to the session (by name).
    Attach,
    /// Send the role command + `\r` as a keystroke.
    Launch,
    /// Wait for the agent's TUI to render.
    WaitTuiReady,
    /// Paste the prompt body as raw input.
    PastePrompt,
    /// (Best-effort) wait for the paste to echo back into the
    /// rolling buffer.
    WaitEcho,
    /// Press Enter to submit the pasted prompt.
    Submit,
    /// Wait for the agent to start processing.
    WaitProcessing,
}

/// Errors from [`dispatch`]. Variants name the step that failed, so
/// callers can map failures back onto progress UIs without parsing.
#[derive(Debug, Error)]
pub enum DispatchError {
    /// `KatulongAsyncClient::new` failed (bad URL, reqwest builder
    /// rejected the config). Should only happen on dev-environment
    /// misconfig, never in production.
    #[error("client setup: {0}")]
    ClientSetup(#[source] anyhow::Error),

    /// Worktree setup `/exec` failed.
    #[error("worktree setup failed: {0}")]
    WorktreeSetup(#[source] KatulongAsyncError),

    /// WebSocket attach open failed.
    #[error("attach failed: {0}")]
    Attach(#[source] AttachError),

    /// Sending the launch keystroke failed.
    #[error("launch keystroke failed: {0}")]
    Launch(#[source] AttachError),

    /// TUI-ready wait failed (timeout, session exited, etc.).
    #[error("TUI ready wait failed: {0}")]
    WaitTuiReady(#[source] AttachError),

    /// Sending the prompt paste failed.
    #[error("paste failed: {0}")]
    PastePrompt(#[source] AttachError),

    /// Pressing Enter failed.
    #[error("submit failed: {0}")]
    Submit(#[source] AttachError),

    /// Processing wait failed (timeout, session exited, etc.).
    #[error("processing wait failed: {0}")]
    WaitProcessing(#[source] AttachError),
}

/// Run one dispatch action end-to-end against a pre-created session.
///
/// The caller is responsible for creating the session
/// (`KatulongAsyncClient::create_dispatch_session`) and any
/// pre-flight checks (the gate, persisting `dispatch_session_id` on
/// the task, etc.) *before* calling this function. This function
/// owns the "set up worktree + attach + launch + paste + submit"
/// sequence — the action proper.
///
/// On failure returns a [`DispatchError`] naming the step that
/// failed; partial work is NOT rolled back. The katulong session
/// passed in is left in place so an operator can inspect it after a
/// failed dispatch.
///
/// The `on_step` callback fires before each step. It runs on the
/// task's tokio runtime, so callers should keep it cheap (publishing
/// a small JSON envelope, printing one line, etc.) — long-running
/// work in the callback blocks the dispatch.
///
/// See the module-level docs for the broader context (what this
/// function is NOT — namely, not the gate, not the nudge loop, not
/// session lifecycle policy).
pub async fn dispatch<F: FnMut(DispatchStep)>(
    remote: RemoteConfig,
    session: &TmuxSession,
    input: DispatchInput,
    mut on_step: F,
) -> Result<(), DispatchError> {
    info!(
        task = input.task_id,
        project = %input.project_name,
        host = %remote.url,
        session_id = %session.id,
        session_name = %session.name,
        "dispatch: start",
    );

    // Step 1: optional worktree setup (HTTP `/exec`).
    if let Some(wt) = &input.worktree {
        on_step(DispatchStep::WorktreeSetup);
        let http = KatulongAsyncClient::new(remote.url.clone(), remote.api_key.clone())
            .map_err(DispatchError::ClientSetup)?;
        http.exec_session(&session.id, &wt.setup_command)
            .await
            .map_err(DispatchError::WorktreeSetup)?;
        info!(task = input.task_id, "dispatch: worktree setup complete");
    }

    // Step 2: WS attach.
    on_step(DispatchStep::Attach);
    let attach_client = KatulongAttachClient::new(remote);
    let attach = attach_client
        .attach(&session.name, DEFAULT_ATTACH_COLS, DEFAULT_ATTACH_ROWS)
        .await
        .map_err(DispatchError::Attach)?;
    info!(task = input.task_id, "dispatch: WS attach open");

    // Run remaining steps under a guard that always closes the
    // attach (success or failure).
    let result = run_attach_flow(&attach, &input, &mut on_step).await;
    attach.close().await;
    result?;

    info!(task = input.task_id, "dispatch: complete");
    Ok(())
}

/// Steps 4-9 of the dispatch — everything that runs over the open
/// WS attach. Split into its own fn so the caller can ALWAYS close
/// the attach on the way out, success or failure.
async fn run_attach_flow<F: FnMut(DispatchStep)>(
    attach: &KatulongAttach,
    input: &DispatchInput,
    on_step: &mut F,
) -> Result<(), DispatchError> {
    // Snapshot the stripped-buffer offset BEFORE sending the launch
    // keystroke. The TUI-ready wait starts matching from this offset
    // so we don't miss a fast render that lands in the gap between
    // `input(...)` returning and `wait_for(...)` registering. See
    // `WaitFrom::FromOffset`.
    let pre_launch_offset = attach.stripped_offset().await;

    // Step 4: launch. When a worktree spec is set, prepend
    // `cd <path> && ` so the agent starts inside the worktree, not
    // wherever the session's shell landed after the setup command.
    // (The setup command itself only creates the worktree dir; it
    // doesn't necessarily cd into it. Forgetting this prepend
    // silently regresses worktree isolation — the agent edits the
    // wrong tree.)
    on_step(DispatchStep::Launch);
    let launch = match input.worktree.as_ref() {
        Some(wt) => format!("cd {} && {}\r", wt.path, input.role_command),
        None => format!("{}\r", input.role_command),
    };
    attach.input(&launch).await.map_err(DispatchError::Launch)?;

    // Step 5: wait for TUI to render.
    on_step(DispatchStep::WaitTuiReady);
    attach
        .wait_for(
            tui_ready_re(),
            WaitFrom::FromOffset(pre_launch_offset),
            Some(TUI_READY_TIMEOUT),
        )
        .await
        .map_err(DispatchError::WaitTuiReady)?;

    // Step 6: paste prompt body.
    on_step(DispatchStep::PastePrompt);
    attach
        .input(input.prompt.clone())
        .await
        .map_err(DispatchError::PastePrompt)?;

    // Step 7: best-effort echo wait. We match the *task title* (the
    // unique-per-dispatch portion of the prompt) rather than the
    // prompt prefix — the prompt body always starts with the same
    // `## Context` header, which would degenerate to a no-op match
    // in any rolling buffer that still has prior dispatch content.
    // A missed echo is logged and dispatch proceeds to submit.
    on_step(DispatchStep::WaitEcho);
    if let Ok(re) = paste_echo_regex(&input.task_title) {
        if let Err(e) = attach
            .wait_for(&re, WaitFrom::FromNow, Some(PASTE_ECHO_TIMEOUT))
            .await
        {
            warn!(
                task = input.task_id,
                error = %e,
                "dispatch: paste echo not observed; proceeding to submit",
            );
        }
    }

    // Step 8: submit.
    on_step(DispatchStep::Submit);
    attach
        .press(KeyName::Enter)
        .await
        .map_err(DispatchError::Submit)?;

    // Step 9: wait for processing.
    on_step(DispatchStep::WaitProcessing);
    attach
        .wait_for(
            claude_processing_re(),
            WaitFrom::FromNow,
            Some(PROCESSING_TIMEOUT),
        )
        .await
        .map_err(DispatchError::WaitProcessing)?;

    Ok(())
}

/// Build a regex that matches the first ~20 visible chars of the
/// supplied string — used to confirm the paste echoed into the
/// agent's input box. Escapes regex metacharacters so an `^` or `*`
/// in the title doesn't blow up the matcher. Returns `Err` if the
/// trimmed prefix is empty (which would compile to a regex that
/// matches every position and silently turn the echo wait into a
/// no-op).
fn paste_echo_regex(s: &str) -> Result<regex::Regex, regex::Error> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(regex::Error::Syntax("empty echo source".to_string()));
    }
    let prefix: String = trimmed.chars().take(20).collect();
    let escaped = regex::escape(prefix.trim());
    if escaped.is_empty() {
        return Err(regex::Error::Syntax(
            "empty echo prefix after trim".to_string(),
        ));
    }
    regex::Regex::new(&escaped)
}

/// Pattern matching Claude Code's "ready for input" signal. We
/// match the help hint, the version banner, or the bottom-of-pane
/// prompt indicator (`> ` at end of buffer). We deliberately do
/// NOT include `"esc to interrupt"` — that's the BUSY indicator
/// (matched by [`claude_processing_re`]), and a stale match would
/// resolve the ready-wait against the previous dispatch's tail.
///
/// `expect()` is fine: the pattern is a compile-time literal and a
/// failure here is a developer bug surfaced by the test suite.
fn tui_ready_re() -> &'static regex::Regex {
    static CELL: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        regex::Regex::new(r"/help|Claude Code|>\s*$").expect("tui_ready_re compiles")
    })
}

/// Pattern matching Claude Code's "I'm processing your request"
/// indicator. `"esc to interrupt"` is the load-bearing string that
/// only appears while Claude is actively running a tool / streaming
/// a response.
fn claude_processing_re() -> &'static regex::Regex {
    static CELL: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        regex::Regex::new(r"esc to interrupt").expect("claude_processing_re compiles")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_echo_regex_matches_title_prefix() {
        let re = paste_echo_regex("Fix the dispatch race").expect("compiles");
        assert!(re.is_match("...some prefix Fix the dispatch race trailing"));
    }

    #[test]
    fn paste_echo_regex_escapes_meta() {
        // `*` would be a quantifier without escape; `regex::escape`
        // turns it into a literal match.
        let re = paste_echo_regex("** start! [bug]").expect("compiles");
        assert!(re.is_match("pasted: ** start! [bug] continues"));
    }

    #[test]
    fn paste_echo_regex_empty_returns_err() {
        // Vacuous regex (matches every position) would silently turn
        // the echo wait into a no-op — refuse to compile it.
        assert!(paste_echo_regex("").is_err());
        assert!(paste_echo_regex("   ").is_err());
    }

    #[test]
    fn paste_echo_regex_takes_at_most_20_chars() {
        // A very long title shouldn't anchor on the whole thing;
        // 20 chars is enough to be distinctive but short enough to
        // round-trip through the agent's input box quickly.
        // Asserts both the structural cap AND the behavioral
        // semantics — the first 20 chars match, but the suffix does
        // NOT, so a refactor that silently changes `take(20)` to
        // `take(0)` or `take(100)` is caught.
        let title = "abcdefghijklmnopqrstuvwxyz"; // 26 chars
        let re = paste_echo_regex(title).expect("compiles");
        let pattern = re.as_str();
        // The pattern is a regex-escaped prefix; the source prefix
        // is 20 chars, so the pattern is at most ~40 chars after
        // worst-case escape doubling.
        assert!(
            pattern.len() <= 40,
            "pattern unexpectedly long: {} chars: {pattern}",
            pattern.len()
        );
        // First 20 chars match.
        assert!(
            re.is_match("...abcdefghijklmnopqrst..."),
            "expected match on first-20 prefix: {pattern}"
        );
        // The trailing suffix `uvwxyz` does NOT match on its own —
        // pins the cap (a refactor changing 20→0 would silently
        // produce a vacuous regex; one changing 20→100 would
        // anchor on the full title).
        assert!(
            !re.is_match("uvwxyz"),
            "expected no match on trailing suffix: {pattern}"
        );
    }

    #[test]
    fn tui_ready_re_matches_known_signals() {
        let re = tui_ready_re();
        assert!(re.is_match("Type /help for more"));
        assert!(re.is_match("Claude Code v1.2.3"));
        assert!(re.is_match("...pane bottom\n> "));
    }

    #[test]
    fn tui_ready_re_does_not_match_busy_signal() {
        // The processing indicator must NOT match the ready regex —
        // otherwise a stale "esc to interrupt" in the rolling buffer
        // would falsely resolve the ready wait.
        let re = tui_ready_re();
        assert!(!re.is_match("press esc to interrupt"));
    }

    #[test]
    fn claude_processing_re_matches_busy_signal() {
        let re = claude_processing_re();
        assert!(re.is_match("Bash(ls)... esc to interrupt"));
    }

    #[test]
    fn dispatch_input_is_clone() {
        // Pin the trait bounds — callers want to construct an input
        // once and pass it to dispatch by value, possibly more than
        // once (retry loops).
        let i = DispatchInput {
            task_id: 1,
            project_name: "x".into(),
            task_title: "y".into(),
            role_command: "claude".into(),
            prompt: "p".into(),
            worktree: None,
        };
        let _clone = i.clone();
    }

    #[test]
    fn worktree_spec_is_clone() {
        let w = WorktreeSpec {
            setup_command: "cd /tmp && true".into(),
            path: "/tmp".into(),
        };
        let _clone = w.clone();
    }

    #[test]
    fn dispatch_step_is_copy() {
        // Step is `Copy` so `on_step` callbacks can take it by
        // value without lifetime gymnastics.
        let s = DispatchStep::Attach;
        let _s2 = s;
        let _s3 = s;
    }
}
