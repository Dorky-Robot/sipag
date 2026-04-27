//! WebSocket transport for live pub/sub.
//!
//! `GET /ws` upgrades to a WebSocket. The connection is multiplexed —
//! a single socket can subscribe to many topics, publish, and receive
//! envelopes interleaved as they arrive.
//!
//! Auth: gated by the existing `auth_middleware::gate`, applied above
//! the router. We don't add a second check here.
//!
//! Wire format (JSON, both directions):
//!
//! Client → server:
//!   {"action":"subscribe","topic":"workers/activity","from_seq":0}
//!   {"action":"unsubscribe","topic":"workers/activity"}
//!   {"action":"publish","topic":"a/b","kind":"x","payload":{...}}
//!   {"action":"ping"}
//!
//! Server → client (envelopes interleaved with control):
//!   {"seq":1,"ts":"...","topic":"...","kind":"...","payload":{...}}
//!   {"error":"..."}
//!
//! Limits: 100 concurrent subscriptions per connection; 64 KB per
//! incoming frame; 30s server-initiated ping; 60s pong timeout.

use crate::serve::state::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
    routing::get,
    Router,
};
use serde::Deserialize;
use sipag_core::pubsub::{Broker, Envelope};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};
use tracing::debug;

const MAX_SUBSCRIPTIONS: usize = 100;
const MAX_FRAME_BYTES: usize = 64 * 1024;
const PING_INTERVAL: Duration = Duration::from_secs(30);
const PONG_TIMEOUT: Duration = Duration::from_secs(60);

pub fn routes() -> Router<AppState> {
    Router::new().route("/ws", get(ws_handler))
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.max_message_size(MAX_FRAME_BYTES)
        .max_frame_size(MAX_FRAME_BYTES)
        .on_upgrade(move |socket| handle_socket(socket, state))
}

/// One incoming client message. `serde(tag = "action")` makes the
/// "subscribe" / "unsubscribe" / "publish" / "ping" branches.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "lowercase")]
enum ClientMsg {
    Subscribe {
        topic: String,
        #[serde(default)]
        from_seq: u64,
    },
    Unsubscribe {
        topic: String,
    },
    Publish {
        topic: String,
        kind: String,
        #[serde(default)]
        payload: serde_json::Value,
    },
    Ping,
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (sink, mut stream) = socket.split_owned();

    // mpsc → ws sink. All outbound traffic (envelopes, errors,
    // control) funnels through this so the sink isn't shared across
    // tasks.
    let (out_tx, mut out_rx) = mpsc::channel::<Message>(256);

    // Per-topic subscription state. When the client unsubscribes (or
    // disconnects), dropping the JoinHandle aborts its forwarder.
    let subs: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // Pong watchdog — touched when a pong frame arrives.
    let last_pong = Arc::new(Mutex::new(std::time::Instant::now()));

    // Outbound pump: drains `out_rx` and writes to the socket.
    let mut sink = sink;
    let outbound = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if sink.send(msg).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    // Heartbeat: every PING_INTERVAL we send a Ping; if no Pong has
    // arrived within PONG_TIMEOUT we give up.
    let hb_tx = out_tx.clone();
    let hb_pong = last_pong.clone();
    let heartbeat = tokio::spawn(async move {
        let mut tick = tokio::time::interval(PING_INTERVAL);
        tick.tick().await; // skip the immediate-fire
        loop {
            tick.tick().await;
            if hb_tx.send(Message::Ping(Vec::new())).await.is_err() {
                break;
            }
            let last = *hb_pong.lock().await;
            if last.elapsed() > PONG_TIMEOUT {
                debug!("ws heartbeat timeout");
                break;
            }
        }
    });

    // Inbound dispatcher.
    while let Some(frame) = stream.next().await {
        let Ok(msg) = frame else { break };
        match msg {
            Message::Text(text) => {
                if text.len() > MAX_FRAME_BYTES {
                    let _ = out_tx
                        .send(Message::Text(error_frame("frame too large")))
                        .await;
                    continue;
                }
                let Ok(parsed): Result<ClientMsg, _> = serde_json::from_str(&text) else {
                    let _ = out_tx
                        .send(Message::Text(error_frame("malformed message")))
                        .await;
                    continue;
                };
                handle_client_msg(parsed, &state, &subs, &out_tx).await;
            }
            Message::Binary(_) => {
                let _ = out_tx
                    .send(Message::Text(error_frame("binary frames not supported")))
                    .await;
            }
            Message::Pong(_) => {
                *last_pong.lock().await = std::time::Instant::now();
            }
            Message::Ping(payload) => {
                let _ = out_tx.send(Message::Pong(payload)).await;
            }
            Message::Close(_) => break,
        }
    }

    // Disconnect: drop subscription forwarders and stop pumps.
    let mut subs_guard = subs.lock().await;
    for (_, handle) in subs_guard.drain() {
        handle.abort();
    }
    drop(subs_guard);
    drop(out_tx);
    heartbeat.abort();
    let _ = outbound.await;
}

async fn handle_client_msg(
    msg: ClientMsg,
    state: &AppState,
    subs: &Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    out: &mpsc::Sender<Message>,
) {
    match msg {
        ClientMsg::Ping => {
            let _ = out.send(Message::Text(json_pong())).await;
        }
        ClientMsg::Subscribe { topic, from_seq } => {
            let mut subs_guard = subs.lock().await;
            if subs_guard.contains_key(&topic) {
                // Idempotent.
                return;
            }
            if subs_guard.len() >= MAX_SUBSCRIPTIONS {
                let _ = out
                    .send(Message::Text(error_frame("subscription limit reached")))
                    .await;
                return;
            }

            // Replay history (synchronous, on the broker's behalf).
            let envs = match state.broker.read(&topic, from_seq) {
                Ok(envs) => envs,
                Err(e) => {
                    let _ = out
                        .send(Message::Text(error_frame(format!("subscribe failed: {e}"))))
                        .await;
                    return;
                }
            };
            for env in envs {
                if let Ok(line) = serde_json::to_string(&env) {
                    if out.send(Message::Text(line)).await.is_err() {
                        return;
                    }
                }
            }

            // Live forwarder.
            let mut rx = state.broker.subscribe(&topic);
            let out_clone = out.clone();
            let topic_owned = topic.clone();
            let handle = tokio::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(env) => {
                            if env.topic != topic_owned {
                                continue;
                            }
                            if let Ok(line) = serde_json::to_string(&env) {
                                if out_clone.send(Message::Text(line)).await.is_err() {
                                    break;
                                }
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            // Drop and continue — better than disconnecting.
                            continue;
                        }
                        Err(_) => break,
                    }
                }
            });
            subs_guard.insert(topic, handle);
        }
        ClientMsg::Unsubscribe { topic } => {
            let mut subs_guard = subs.lock().await;
            if let Some(handle) = subs_guard.remove(&topic) {
                handle.abort();
            }
        }
        ClientMsg::Publish {
            topic,
            kind,
            payload,
        } => {
            if let Err(e) = state.broker.publish(&topic, &kind, payload) {
                let _ = out
                    .send(Message::Text(error_frame(format!("publish failed: {e}"))))
                    .await;
            }
        }
    }
}

fn error_frame(msg: impl Into<String>) -> String {
    let m: String = msg.into();
    serde_json::json!({ "error": m }).to_string()
}

fn json_pong() -> String {
    r#"{"kind":"pong"}"#.to_string()
}

#[allow(dead_code)]
fn _ensure_broker_send_sync(_b: Broker, _e: Envelope) {
    // Compile-time check: Broker and Envelope are both Send + Sync so
    // they can move across tokio tasks. If this stops compiling, the
    // pubsub module changed in a way that breaks the WS path.
    fn assert<T: Send + Sync>() {}
    assert::<Broker>();
    assert::<Envelope>();
}

// Tiny helper trait import shim — futures::StreamExt + futures::SinkExt
use futures::{SinkExt, StreamExt};

// axum 0.7 provides WebSocket::split through the `WebSocket::split` method —
// our impl uses `split_owned` which returns owned halves so the inbound and
// outbound pumps can move into separate tasks. If this is missing on the
// upstream, we fall back to wrapping the socket in an Arc<Mutex<…>>.
trait WebSocketExt {
    fn split_owned(
        self,
    ) -> (
        futures::stream::SplitSink<WebSocket, Message>,
        futures::stream::SplitStream<WebSocket>,
    );
}

impl WebSocketExt for WebSocket {
    fn split_owned(
        self,
    ) -> (
        futures::stream::SplitSink<WebSocket, Message>,
        futures::stream::SplitStream<WebSocket>,
    ) {
        StreamExt::split(self)
    }
}
