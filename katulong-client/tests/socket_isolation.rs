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
//! is isolated. This test asserts the property end-to-end:
//!
//!   1. Run two harnesses on two different sockets.
//!   2. Create a session on A and open a WS attach to it.
//!   3. Wait past katulong's 5s child-count-monitor tick so any
//!      adopt-from-socket path on B would have fired.
//!   4. Assert (a) A still lists the session, (b) B never saw it,
//!      and — critically — (c) A's *attach* is still healthy by
//!      sending input through it. With the bug, B's adopt would have
//!      detached A's control client and A's attach would surface
//!      `SessionExited(0)` here instead of `Ok`.
//!
//! Requires `KATULONG_REPO=…`; skips cleanly otherwise.

mod common;

use std::time::Duration;

use katulong_client::{KatulongAttachClient, KatulongClient, RemoteConfig};

#[tokio::test]
async fn two_harnesses_with_distinct_sockets_do_not_share_sessions() {
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
    let http_b = KatulongClient::new(b.url(), api_key.clone());

    // Create a session on A and open a WS attach. With the bug, B's
    // adoption of this `kat_<id>` (via the shared tmux socket) would
    // detach A's control client mid-test — and *this* attach, not
    // just A's session list, would be the canary.
    let session = http_a.create_dispatch_session().expect("create on A");
    let attach_client = KatulongAttachClient::new(RemoteConfig {
        url: a.url(),
        api_key,
    });
    let attach = attach_client
        .attach(&session.name, 120, 40)
        .await
        .expect("attach on A");

    // Wait past katulong's 5s child-count-monitor tick so any
    // adopt-from-socket path on B would have fired.
    tokio::time::sleep(Duration::from_secs(6)).await;

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

    // The strict assertion: A's WS attach is still alive. With the
    // bug this errors with `SessionExited(0)` — that was the original
    // symptom the operator saw at the notebook layer.
    attach
        .input(" ".to_string())
        .await
        .expect("attach on A is still healthy — B did not detach our control client");

    attach.close().await;
}
