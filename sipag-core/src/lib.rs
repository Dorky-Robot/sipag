pub mod auth;
/// OKR + Task + Role + Project + Observation domain types. Extracted
/// into its own workspace crate (`sipag-board`). Re-exported here so
/// existing `sipag_core::board::…` imports keep compiling during the
/// transition. Direct `sipag_board::…` imports are preferred for new
/// code.
pub use sipag_board as board;
pub mod config;
/// ⛔ Deprecated 2026-05-17. Kanban-shaped refinement pipeline (raw
/// idea → grouped → refined → ticket) — replaced by the Experimentation
/// context (spike → observe → iterate). Source preserved as "we tried
/// this" per memory `feedback-deprecate-with-rationale`. See
/// `docs/modules.md` §3 and memory
/// `project-sipag-work-model-experimentation` for rationale.
///
/// The `#[deprecated]` attribute is intentionally omitted: no in-tree
/// caller depends on this module (CLI subcommands were stripped, no
/// `serve/` or `tui/` references), and the attribute would only fire on
/// out-of-tree consumers we'd rather not surprise mid-revision. The
/// human-visible deprecation banner inside `feature.rs` is the
/// discoverability mechanism. Add `#[deprecated]` back if the module
/// ever picks up a new caller you want a compile-time warning on —
/// **important**: [`refine`] also uses [`feature::Feature`], so they
/// must be deprecated as a unit (otherwise the test-harness friendly
/// fire from `#[deprecated]` returns).
pub mod feature;
pub mod gate;
/// Katulong wire protocol + HTTP/WS client. Extracted into its own
/// workspace crate (`katulong-client`) so it can be developed and
/// tested in isolation from sipag-the-task-manager. Re-exported here
/// so existing `sipag_core::katulong::…` imports keep compiling
/// during the transition. Direct `katulong_client::…` imports are
/// preferred for new code.
pub use katulong_client as katulong;
/// Host topology config (`~/.sipag/hosts.toml`). Extracted into its
/// own workspace crate (`sipag-mesh`) so it can be developed and
/// tested in isolation. Re-exported here so existing
/// `sipag_core::hosts::…` imports keep compiling during the
/// transition. Direct `sipag_mesh::…` imports are preferred for
/// new code.
pub use sipag_mesh as hosts;
pub mod llm;
pub mod nudge;
/// File-backed durable pub/sub broker. Extracted into its own workspace
/// crate (`sipag-pubsub`) so it can be developed and tested in
/// isolation. Re-exported here so existing `sipag_core::pubsub::…`
/// imports keep compiling during the transition. Direct
/// `sipag_pubsub::…` imports are preferred for new code.
pub use sipag_pubsub as pubsub;
/// ⛔ Deprecated 2026-05-17. Companion to [`feature`] — the batch
/// refiner that turns raw features into actionable tickets via a
/// `claude` subprocess. Replaced by the Experimentation context. Source
/// preserved per memory `feedback-deprecate-with-rationale`. See
/// [`feature`]'s doc above for the full rationale on why
/// `#[deprecated]` itself is omitted.
pub mod refine;
