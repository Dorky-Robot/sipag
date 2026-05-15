//! Pins katulong's session-name-vs-session-id lookup semantics, the
//! protocol-level shape of the bug fixed in PR #534.
//!
//! Katulong's WS attach handler keys sessions by NAME
//! (`sessions.get(name)`). When a caller passes the session ID
//! where the name was expected, katulong's `attachClient` **silently
//! spawns a new session named after the id** rather than rejecting
//! the request. Sipag's v2 dispatch path hit this and typed
//! `claude\r` into a phantom session while the operator watched the
//! original sit empty.
//!
//! These tests:
//! - Confirm that attaching with `session.name` does NOT spawn a
//!   phantom — the session count stays at 1.
//! - Confirm that attaching with `session.id` (the bug) DOES spawn
//!   a phantom — so if katulong ever tightens this to reject
//!   unknown names, we'll notice immediately.

mod common;

use common::{KatulongHarness, SKIP_MSG};
use katulong_client::{KatulongAttachClient, KatulongClient, RemoteConfig};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attach_by_name_does_not_spawn_phantom() {
    let Some(harness) = KatulongHarness::try_start().expect("spawn katulong") else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let api_key = "unused-because-localhost".to_string();
    let http = KatulongClient::new(harness.url(), api_key.clone());
    let session = http.create_dispatch_session().expect("create session");

    // Sanity: katulong knows about exactly one session right now.
    let before = http.list_sessions().expect("list");
    assert_eq!(before.len(), 1, "expected fresh katulong to have 1 session");
    assert_eq!(before[0].id, session.id);

    let attach_client = KatulongAttachClient::new(RemoteConfig {
        url: harness.url(),
        api_key,
    });
    let attach = attach_client
        .attach(&session.name, 120, 40)
        .await
        .expect("WS attach by name");
    attach.close().await;

    // After attaching by NAME, the session count must still be 1.
    // If a regression swaps `&session.name` for `&session.id` at the
    // call site, katulong silently spawns a second session and the
    // count becomes 2.
    let after = http.list_sessions().expect("list");
    assert_eq!(
        after.len(),
        1,
        "attaching by name spawned a phantom session: {after:?}",
    );
    assert_eq!(after[0].id, session.id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attach_by_id_spawns_phantom_session_with_id_as_name() {
    // This is the bug shape: we DOCUMENT that katulong currently
    // spawns a phantom when given an id-as-name. If a future
    // katulong release tightens this (e.g., rejects unknown
    // session names on attach), this test fails and we know to
    // update our expectations / remove the failsafe defensive
    // guard in sipag.
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
    // Deliberately attach with the ID where the name was expected
    // — reproduces the PR #534 bug shape.
    let attach = attach_client
        .attach(&session.id, 120, 40)
        .await
        .expect("WS attach by id (katulong currently accepts this and spawns a phantom)");
    attach.close().await;

    let after = http.list_sessions().expect("list");
    let phantom = after.iter().find(|s| s.name == session.id);
    assert!(
        phantom.is_some(),
        "attaching by id no longer spawns a phantom session — sipag's defensive `&session.name` \
         guard may be safely tightened. Sessions after attach: {after:?}",
    );
    assert!(
        after.len() >= 2,
        "expected at least 2 sessions (original + phantom), got {}: {after:?}",
        after.len(),
    );
}
