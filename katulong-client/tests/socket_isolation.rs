//! Regression test for the "two katulong instances on the default
//! tmux socket adopt each other's sessions" bug.
//!
//! Symptom (without isolation): operator runs the sandbox while their
//! daily-driver katulong is also up on another port. The daily driver
//! discovers the sandbox's freshly-spawned `kat_<id>` session on the
//! shared default tmux socket, adopts it as external, opens its OWN
//! `tmux -C attach-session -d -t kat_<id>` — the `-d` detaches the
//! sandbox's control client, the sandbox's session reports exit code 0
//! to the Rust attach, and the next `/api/input` fails with
//! `"session ended with exit code 0"`.
//!
//! Fix: each katulong subprocess we spawn (serve, sandbox, harness)
//! sets `KATULONG_TMUX_SOCKET` to a unique value so its tmux server
//! is isolated. This test asserts the property end-to-end by running
//! two harnesses on two different sockets and verifying neither sees
//! the other's sessions even after a full child-count-monitor tick
//! (5s in katulong).
//!
//! Requires `KATULONG_REPO=…`; skips cleanly otherwise.

mod common;

use std::time::Duration;

use katulong_client::KatulongClient;

#[test]
fn two_harnesses_with_distinct_sockets_do_not_share_sessions() {
    let Ok(Some(a)) = common::KatulongHarness::try_start() else {
        eprintln!("KATULONG_REPO not set or invalid — skipping");
        return;
    };
    let Ok(Some(b)) = common::KatulongHarness::try_start() else {
        eprintln!("KATULONG_REPO not set or invalid — skipping");
        return;
    };

    let api_key = "unused-because-localhost".to_string();
    let http_a = KatulongClient::new(a.url(), api_key.clone());
    let http_b = KatulongClient::new(b.url(), api_key);

    // Create a session via harness A. With the bug, harness B would
    // discover this session on the shared default tmux socket within
    // ~5 seconds and either adopt it (showing up in /sessions) or
    // detach A's control client (causing A to lose track of it).
    let session = http_a.create_dispatch_session().expect("create on A");

    // Wait past katulong's 5s child-count-monitor tick so any
    // adopt-from-socket path on B would have fired.
    std::thread::sleep(Duration::from_secs(6));

    let on_a = http_a.list_sessions().expect("list A");
    let on_b = http_b.list_sessions().expect("list B");

    assert!(
        on_a.iter().any(|s| s.name == session.name),
        "A lost its own session — control client may have been detached \
         by another katulong sharing the default tmux socket. Sessions on A: {on_a:?}"
    );
    assert!(
        !on_b.iter().any(|s| s.id == session.id),
        "B sees a session it never created — the two katulong instances are sharing \
         a tmux socket. Sessions on B: {on_b:?}"
    );
}
