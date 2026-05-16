//! End-to-end smoke: drive a `KatulongAttachClient` against a real
//! katulong subprocess. The original "everything works together"
//! test that surfaced the four prod bugs in PR #534.
//!
//! Activated via `KATULONG_REPO=/path/to/katulong-checkout`. Skips
//! cleanly when unset so CI without a katulong checkout stays
//! green.

mod common;

use common::{KatulongHarness, SKIP_MSG};
use katulong_client::{KatulongAttachClient, KatulongClient, RemoteConfig, WaitFrom};
use std::time::Duration;

/// Bootstrap test: drive `KatulongAttachClient` against a real
/// katulong subprocess. Validates the full WS-level handshake, the
/// session-name lookup path (not session-id), the inbound-message
/// parsing (state-check with integer fingerprint), and the
/// keystroke round-trip (input → PTY → output → rolling buffer →
/// `wait_for` match).
///
/// Uses a shell `printf` rather than a `claude` launch so the test
/// is shell-only and doesn't depend on a claude binary on PATH.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attach_input_round_trip_against_real_katulong() {
    let Some(harness) = KatulongHarness::try_start().expect("spawn katulong") else {
        eprintln!("{SKIP_MSG}");
        return;
    };

    // Localhost auth bypass — katulong treats 127.0.0.1 callers as
    // authenticated, so we don't need to mint an API key for the
    // happy path. A non-local Origin test will need its own setup.
    let api_key = "unused-because-localhost".to_string();
    let http = KatulongClient::new(harness.url(), api_key.clone());
    let session = http.create_dispatch_session().expect("create session");

    let attach_client = KatulongAttachClient::new(RemoteConfig {
        url: harness.url(),
        api_key,
    });

    let attach = attach_client
        .attach(&session.name, 120, 40)
        .await
        .expect("WS attach");

    // Unique sentinel so a shell prompt or banner can't satisfy
    // the wait by accident.
    let token = "v2-dispatch-roundtrip-sentinel";
    let cmd = format!("printf '%s\\n' '{token}'\r");
    let offset_before = attach.stripped_offset().await;
    attach.input(&cmd).await.expect("input(echo)");

    let pat = regex::Regex::new(token).expect("compile");
    let m = attach
        .wait_for(
            &pat,
            WaitFrom::FromOffset(offset_before),
            Some(Duration::from_secs(10)),
        )
        .await
        .expect("wait_for token after input round-trip");

    assert_eq!(m.matched_text, token);

    // Close MUST terminate quickly — wrap in a timeout so a
    // regression to the buggy reader/writer/close ordering (where
    // the reader's `writer_tx.clone()` keeps the channel open and
    // the writer task hangs forever on `rx.recv()`) fails the test
    // loudly instead of hanging it.
    tokio::time::timeout(Duration::from_secs(2), attach.close())
        .await
        .expect("close hung — likely a writer-task deadlock regression");
}
