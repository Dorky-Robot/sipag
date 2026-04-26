//! sipag binary's library surface.
//!
//! main.rs is a thin entry point that delegates to `cli::run`. The
//! modules declared here are reachable from integration tests in
//! `tests/`, so we can drive `serve::build_router` against an
//! in-process router (axum-test) without binding a real port.

pub mod cli;
pub mod serve;
