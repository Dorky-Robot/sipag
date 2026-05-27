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

pub mod async_http;
pub mod attach;
pub mod http;
pub mod protocol;
pub mod sse;

/// Notebook-style web UI for stepping through library calls. Pulls
/// in axum and a small JSON surface; gated behind the `serve` Cargo
/// feature so headless library consumers can skip axum's compile
/// cost via `default-features = false`. Enabled by default for the
/// `katulong-client` binary.
#[cfg(feature = "serve")]
pub mod serve;

// Re-export the HTTP REST surface at the crate root.
pub use http::{
    agent_command, claude_respond_url, claude_transcript_url, exec_url,
    generate_dispatch_session_name, is_valid_session_id, kill_url, output_lines_url, session_name,
    sessions_url, status_url, worktree_branch, worktree_command, worktree_path, KatulongClient,
    RemoteConfig, TmuxSession, TmuxSessionStatus,
};

// Re-export the most common attach types so callers don't need to
// reach into `attach::` for the happy path.
pub use attach::{
    AttachError, AttachResult, KatulongAttach, KatulongAttachClient, KeyName, RegexMatch, WaitFrom,
};

// Re-export the async HTTP client at the crate root. Sibling of
// `KatulongClient` (sync) and `KatulongAttachClient` (WS); pair them
// based on what the caller needs (see async_http.rs module docs).
pub use async_http::{
    bytes_capped, AsyncResult, KatulongAsyncClient, KatulongAsyncError, DEFAULT_BODY_CAP,
    TRANSCRIPT_BODY_CAP,
};

// Re-export the SSE subscriber. The third wire surface alongside
// the sync REST client, the async HTTP client, and the WS attach
// client. Feeds the lens-worker bridge (modules.md §9 Phase 1 #3 +
// Phase 2 #7) — emits structured `KatulongEvent`s the bridge's
// sliding window consumes.
pub use sse::{subscribe, KatulongEvent, KatulongEventStream, SseError};
