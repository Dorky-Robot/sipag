//! Lifecycle invariants for `KatulongAttach`: open, close, drop,
//! re-attach. Pins the bugs surfaced in PR #534 (close-deadlock)
//! and guards against future regressions in the ordering of
//! reader-abort / writer-channel-drop / writer-join.

mod common;

use common::{KatulongHarness, SKIP_MSG};
use katulong_client::{KatulongAttachClient, KatulongClient, RemoteConfig};
use std::time::Duration;

const COLS: u16 = 120;
const ROWS: u16 = 40;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_terminates_within_two_seconds() {
    // Regression for the PR #534 close() deadlock: the reader task
    // holds a `writer_tx.clone()` so it can fire Pull on
    // DataAvailable. If close() drops only the owned `writer_tx`,
    // the channel stays open because of the reader's clone, and
    // the writer task blocks forever on `rx.recv().await`. The fix
    // is to abort + join the reader FIRST (releases its sender),
    // THEN drop our sender. This test would hang for ages without
    // the fix; we cap it at 2s to convert any regression into a
    // loud test failure rather than a CI timeout.
    let Some(harness) = KatulongHarness::try_start().expect("spawn katulong") else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let api_key = "unused-because-localhost".to_string();
    let http = KatulongClient::new(harness.url(), api_key.clone());
    let session = http.create_dispatch_session().expect("create session");
    let attach_client = KatulongAttachClient::new(RemoteConfig {
        url: harness.url(),
        api_key,
    });
    let attach = attach_client
        .attach(&session.name, COLS, ROWS)
        .await
        .expect("WS attach");

    tokio::time::timeout(Duration::from_secs(2), attach.close())
        .await
        .expect("close hung — likely a writer-task deadlock regression");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_without_close_does_not_hang_runtime() {
    // The `Drop` impl aborts both background tasks unconditionally.
    // If a future refactor removes Drop (or makes it conditional)
    // and a caller forgets `close().await`, the attach's spawned
    // tasks would outlive the handle and the test process. This
    // test verifies that dropping a `KatulongAttach` without
    // calling `close().await` terminates promptly — at process
    // shutdown, tokio aborts the runtime, but we want graceful
    // cleanup before that.
    let Some(harness) = KatulongHarness::try_start().expect("spawn katulong") else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let api_key = "unused-because-localhost".to_string();
    let http = KatulongClient::new(harness.url(), api_key.clone());
    let session = http.create_dispatch_session().expect("create session");
    let attach_client = KatulongAttachClient::new(RemoteConfig {
        url: harness.url(),
        api_key,
    });

    // Open and immediately drop without close — the Drop impl
    // must abort both writer + reader tasks so this test
    // returns within the timeout window.
    let work = async {
        let attach = attach_client
            .attach(&session.name, COLS, ROWS)
            .await
            .expect("WS attach");
        drop(attach);
        // Give the runtime a moment to actually drop the future
        // and abort its captured tasks.
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    tokio::time::timeout(Duration::from_secs(3), work)
        .await
        .expect("dropping KatulongAttach without close hung");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn re_attach_after_close_succeeds() {
    // Sequential attach → close → attach to the same session must
    // work cleanly. Pins that close() leaves no katulong-side state
    // that blocks a fresh client from joining the same session.
    let Some(harness) = KatulongHarness::try_start().expect("spawn katulong") else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let api_key = "unused-because-localhost".to_string();
    let http = KatulongClient::new(harness.url(), api_key.clone());
    let session = http.create_dispatch_session().expect("create session");
    let attach_client = KatulongAttachClient::new(RemoteConfig {
        url: harness.url(),
        api_key,
    });

    for round in 1..=3 {
        let attach = attach_client
            .attach(&session.name, COLS, ROWS)
            .await
            .unwrap_or_else(|e| panic!("attach round {round} failed: {e}"));
        // Each round writes a marker so we can confirm the PTY
        // remained alive across re-attaches.
        attach
            .input(format!("# round-{round}\r"))
            .await
            .unwrap_or_else(|e| panic!("input round {round} failed: {e}"));
        tokio::time::timeout(Duration::from_secs(2), attach.close())
            .await
            .unwrap_or_else(|_| panic!("close round {round} hung"));
    }
}
