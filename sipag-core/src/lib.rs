pub mod auth;
pub mod board;
pub mod config;
/// ⛔ Deprecated 2026-05-17. Kanban-shaped refinement pipeline (raw
/// idea → grouped → refined → ticket) — replaced by the Experimentation
/// context (spike → observe → iterate). Source preserved as "we tried
/// this" per memory `feedback-deprecate-with-rationale`. See
/// `docs/modules.md` §3 and memory
/// `project-sipag-work-model-experimentation` for rationale. The
/// `#[deprecated]` attribute is intentionally omitted: external wiring
/// has been stripped (no CLI subcommands), so the attribute's audience
/// (external callers) doesn't exist. The deprecation banner inside
/// `feature.rs` is the human-facing notice; add `#[deprecated]` back if
/// the module ever picks up a new caller you want a compile-time warning
/// on.
pub mod feature;
pub mod gate;
pub mod hosts;
/// Katulong wire protocol + HTTP/WS client. Extracted into its own
/// workspace crate (`katulong-client`) so it can be developed and
/// tested in isolation from sipag-the-task-manager. Re-exported here
/// so existing `sipag_core::katulong::…` imports keep compiling
/// during the transition. Direct `katulong_client::…` imports are
/// preferred for new code.
pub use katulong_client as katulong;
pub mod llm;
pub mod nudge;
pub mod pubsub;
/// ⛔ Deprecated 2026-05-17. Companion to [`feature`] — the batch
/// refiner that turns raw features into actionable tickets via a
/// `claude` subprocess. Replaced by the Experimentation context. Source
/// preserved per memory `feedback-deprecate-with-rationale`. See
/// [`feature`]'s doc above for the full rationale on why
/// `#[deprecated]` itself is omitted.
pub mod refine;
