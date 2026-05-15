//! Headless Rust client for katulong.
//!
//! Speaks katulong's HTTP REST surface (`/sessions`,
//! `/sessions/by-id/:id/output`, …) and its WebSocket attach protocol
//! (the same one the katulong browser app uses) so a Rust caller can
//! drive a tmux-backed PTY session programmatically — create the
//! session, attach to it, send input, wait for output, paste a
//! body, press named keys, snapshot the screen.
//!
//! Designed so it can be developed and validated in isolation from
//! its first consumer (`sipag`). Tests against a real katulong
//! subprocess live in `tests/v2_dispatch.rs`. A CLI binary
//! (`katulong-client`) wraps the same library calls for shell-level
//! exploration — `katulong-client paste <session> '<body>'` does
//! exactly what a browser tab pasting into a session does.
//!
//! ## Top-level re-exports
//!
//! For ergonomic call sites, the most-used types are surfaced at
//! the crate root:
//!
//! ```ignore
//! use katulong_client::{KatulongClient, RemoteConfig};
//! use katulong_client::attach::{KatulongAttachClient, KeyName, WaitFrom};
//! ```
//!
//! The submodules (`http`, `attach`, `protocol`) remain accessible
//! for callers that need the lower-level types.

pub mod attach;
pub mod http;
pub mod protocol;

// Re-export the HTTP REST surface at the crate root.
pub use http::{
    agent_command, claude_respond_url, claude_transcript_url, exec_url,
    generate_dispatch_session_name, is_valid_session_id, kill_url, output_lines_url, session_name,
    sessions_url, status_url, worktree_branch, worktree_command, worktree_path, KatulongClient,
    RemoteConfig, Session, SessionStatus,
};

// Re-export the most common attach types so callers don't need to
// reach into `attach::` for the happy path.
pub use attach::{
    AttachError, AttachResult, KatulongAttach, KatulongAttachClient, KeyName, RegexMatch, WaitFrom,
};
