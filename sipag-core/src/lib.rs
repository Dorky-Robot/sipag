pub mod auth;
pub mod board;
pub mod config;
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
pub mod refine;
