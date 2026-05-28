//! Wire protocol for katulong's per-session attach.
//!
//! These are the JSON messages that flow over katulong's
//! `ClientTransport` abstraction (`katulong/lib/client-transport.js`).
//! The transport can be either WebSocket or WebRTC DataChannel
//! server-side; sipag doesn't care which — it sees one stream of
//! JSON frames in each direction.
//!
//! The browser already speaks this protocol via
//! `katulong/public/lib/input-sender.js` (outbound) and the
//! handlers in `katulong/lib/ws-manager.js` (inbound + outbound
//! dispatch). Sipag is a second kind of client speaking the exact
//! same protocol; for the broader architecture see `docs/architecture.md`
//! §4 (Topology).
//!
//! Every message is a single JSON object on a single transport
//! frame. We model them as tagged enums (`#[serde(tag = "type")]`)
//! so encoding and decoding are both compile-checked. Variants we
//! deliberately ignore in the dispatch path still get
//! deserializer arms so an unknown-but-safe message doesn't error
//! the read loop.

use serde::{Deserialize, Serialize};

/// Messages sipag sends *to* katulong.
///
/// Reference: `katulong/lib/ws-manager.js:329-567` (the
/// `wsMessageHandlers` switch).
///
/// Only the variants the dispatch path needs are modeled here.
/// `subscribe`, `unsubscribe`, `set-tab-icon`, and the WebRTC
/// signaling messages (`rtc-offer`, `rtc-ice-candidate`) are
/// browser-tile concerns and are omitted on purpose — sipag uses
/// one session per attach and doesn't need carousels or P2P
/// signaling.
///
/// `Outbound` is `Serialize` only. The mirror, [`Inbound`], is
/// `Deserialize` only. The asymmetry is deliberate: messages
/// flow one-way per direction (sipag never decodes what it sent;
/// katulong never sends what sipag sends), so each enum only
/// needs the side that matches its role. Tests that need to
/// round-trip an inbound message construct a JSON string by hand
/// rather than re-encoding via serde.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Outbound {
    /// Subscribe to a session. First message after the WS opens.
    ///
    /// Triggers `sessionManager.attachClient` server-side; the
    /// response sequence is `attached` + `seq-init` +
    /// `data-available`. Re-sending `attach` after a reconnect
    /// yields a fresh snapshot — there is no `fromSeq` field on
    /// attach (per `ws-manager.js:330-363`).
    Attach {
        session: String,
        cols: u16,
        rows: u16,
    },
    /// Type bytes into the session's PTY. The bytes are passed
    /// through to tmux as-is via `sessionManager.writeInput`
    /// (`ws-manager.js:403-406`). Bracketed-paste markers,
    /// special keystrokes, and submit Enters all ride in this
    /// one message type — there is no separate "paste" or "key"
    /// message. Distinct keystrokes are conveyed by sending
    /// separate `Input` messages, which become separate
    /// `send-keys -H` calls server-side.
    Input {
        data: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        session: Option<String>,
    },
    /// Set the PTY's reported dimensions. Sipag should send this
    /// after a successful attach so TUI apps reflow correctly.
    Resize {
        cols: u16,
        rows: u16,
        #[serde(skip_serializing_if = "Option::is_none")]
        session: Option<String>,
    },
    /// Pull buffered output starting at `from_seq`. The response
    /// is `pull-response` (data + new cursor), `pull-snapshot`
    /// (when `from_seq` has been evicted from the RingBuffer),
    /// or an empty `pull-response` with an advanced cursor (when
    /// the server is intentionally skipping due to backpressure).
    Pull {
        #[serde(rename = "fromSeq")]
        from_seq: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        session: Option<String>,
    },
    /// Request a fresh snapshot. Used when sipag's local
    /// fingerprint disagrees with the server's `state-check`
    /// fingerprint, which means the buffers have drifted.
    Resync {
        #[serde(skip_serializing_if = "Option::is_none")]
        session: Option<String>,
    },
    /// Application-level heartbeat. Distinct from WS frame-level
    /// ping/pong because DataChannel has no native ping. Sipag
    /// sends one every ~30s and watches for `Pong` to detect
    /// half-open connections.
    Ping,
}

/// Messages sipag receives *from* katulong.
///
/// Reference: emitter sites in `katulong/lib/ws-manager.js:124-237`
/// (broadcasts) and `:329-567` (per-handler responses).
///
/// `serde(other)` on the catch-all means any message type sipag
/// doesn't recognize falls into `Other`, which the read loop logs
/// and ignores. This is the right posture: future katulong
/// features that add new message types won't break sipag's parser.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Inbound {
    // ── attach lifecycle ────────────────────────────────────
    /// Initial buffer snapshot after `attach`. `data` is the
    /// serialized xterm screen state (escape sequences ready to
    /// be written into a terminal to reconstruct what's
    /// on-screen at the moment of attach).
    Attached { session: String, data: String },

    /// Initial cursor position. Always follows `attached` (or
    /// `switched`, or first `subscribe`). Sipag sets its internal
    /// cursor to this value before issuing its first pull.
    #[serde(rename = "seq-init")]
    SeqInit { session: String, seq: u64 },

    /// Acknowledgment of `switch` (no buffer). Not used in the
    /// dispatch path but parsed to avoid an `Other` log entry
    /// if it ever lands.
    Switched { session: String },

    // ── output flow ─────────────────────────────────────────
    /// Server pushed output inline (zero round-trip path).
    /// Apply directly if `from_seq` matches the current cursor;
    /// otherwise wait for the next `pull-response` to fill the
    /// gap. Reference: `ws-manager.js:126-151`.
    Output {
        session: String,
        data: String,
        #[serde(rename = "fromSeq")]
        from_seq: u64,
        cursor: u64,
    },

    /// Lightweight nudge that says "data is available; pull now."
    /// Carries no data. Used (a) when the server is backpressured
    /// and can't push inline, (b) right after attach to cover any
    /// output that arrived during routing setup, (c) periodically
    /// to keep clients flowing. Reference: `ws-manager.js:145`,
    /// `:349`, `:393`.
    #[serde(rename = "data-available")]
    DataAvailable { session: String },

    /// Normal pull response — `data` is the bytes since the
    /// requested `from_seq`; `cursor` is the new head. Empty
    /// `data` with an advanced `cursor` is the
    /// backpressure-skip signal (`ws-manager.js:437-442`).
    #[serde(rename = "pull-response")]
    PullResponse {
        session: String,
        data: String,
        cursor: u64,
    },

    /// Recovery response when `from_seq` has been evicted or the
    /// client asked for `resync` after drift detection. `data` is
    /// a full screen snapshot; sipag must REPLACE its rolling
    /// buffer with this, then advance cursor to `cursor`.
    /// Reference: `ws-manager.js:459-464`, `:490-495`.
    #[serde(rename = "pull-snapshot")]
    PullSnapshot {
        session: String,
        data: String,
        cursor: u64,
    },

    // ── lifecycle / signals ─────────────────────────────────
    /// Session ended. `code` is the exit code (-1 if the PTY
    /// died abnormally). Reference: `ws-manager.js:172`.
    Exit { session: String, code: i32 },

    /// Drift probe broadcast every ~500ms of output quiet. If
    /// the local rolling-buffer fingerprint doesn't match
    /// `fingerprint`, sipag sends `Resync`. Reference:
    /// `ws-manager.js:164-169`.
    #[serde(rename = "state-check")]
    StateCheck {
        session: String,
        // Katulong emits a DJB2 hash (signed 32-bit integer); older
        // ws-manager versions emitted a hex string. Accept either by
        // taking raw JSON — the value is not read on the sipag side
        // (drift detection is deferred).
        fingerprint: serde_json::Value,
        seq: u64,
    },

    /// Session was deleted on the server. Sipag should treat its
    /// attach as terminal. Reference: `ws-manager.js:186`.
    #[serde(rename = "session-removed")]
    SessionRemoved { session: String },

    /// Session was renamed — useful for refreshing the cached
    /// name, but sipag tracks by id so this is mostly an FYI.
    /// Reference: `ws-manager.js:188-190`.
    #[serde(rename = "session-renamed")]
    SessionRenamed { name: String, id: String },

    /// Session metadata changed. The full `data` payload is
    /// `session.toJSON()` — useful for picking up
    /// `meta.claude.uuid` first-appearance and similar.
    /// Reference: `ws-manager.js:192-196`.
    #[serde(rename = "session-updated")]
    SessionUpdated {
        session: String,
        data: serde_json::Value,
    },

    /// Another client (likely a browser) resized the session.
    /// Sipag updates cached dims but doesn't render anything.
    /// Reference: `ws-manager.js:198-202`.
    #[serde(rename = "resize-sync")]
    ResizeSync { cols: u16, rows: u16 },

    /// Server-side error response. Surfaced to the caller.
    /// Reference: many sites including `ws-manager.js:304, 333,
    /// 361, 400, 500, 531, 576`.
    Error { message: String },

    /// Heartbeat ack. Reference: `ws-manager.js:557-559`.
    Pong,

    /// Catch-all for messages sipag doesn't model. The read loop
    /// logs the raw JSON at debug level and continues. This lets
    /// katulong add new message types without breaking sipag.
    #[serde(other)]
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── outbound encoding ───────────────────────────────────

    #[test]
    fn attach_serializes_with_dims() {
        // `attach` is the first message after WS open. Verify
        // the JSON shape matches what the browser sends
        // (per public/lib/transport-layer.js usage in
        // katulong's connection-manager).
        let msg = Outbound::Attach {
            session: "sipag-d-abc123".into(),
            cols: 120,
            rows: 40,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json,
            r#"{"type":"attach","session":"sipag-d-abc123","cols":120,"rows":40}"#
        );
    }

    #[test]
    fn input_omits_session_when_unset() {
        // The browser's input-sender.js includes `session` when
        // it knows which session is active. The dispatch path
        // omits it; katulong falls back to the client's bound
        // session. Verify serde drops the key cleanly.
        let msg = Outbound::Input {
            data: "claude\r".into(),
            session: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"input","data":"claude\r"}"#);
    }

    #[test]
    fn input_includes_session_when_explicit() {
        let msg = Outbound::Input {
            data: "x".into(),
            session: Some("foo".into()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"input","data":"x","session":"foo"}"#);
    }

    #[test]
    fn input_escapes_bracketed_paste_markers_as_unicode_escapes() {
        // The body of a bracketed paste includes literal ESC
        // (0x1B). JSON disallows raw control characters in
        // strings — serde_json emits `\u001b` instead.
        // This test pins that behavior so we never accidentally
        // produce raw-byte JSON that katulong's parser rejects.
        let msg = Outbound::Input {
            data: "\u{001b}[200~hello\u{001b}[201~".into(),
            session: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        // Two backslashes in the Rust source = one literal
        // backslash in the test pattern, matching serde_json
        // output (six chars: backslash u 0 0 1 b).
        assert!(
            json.contains("\\u001b[200~hello\\u001b[201~"),
            "json did not contain escaped BPM markers: {json:?}"
        );
        // And specifically does NOT contain a raw ESC byte.
        assert!(!json.contains('\u{001b}'));
    }

    #[test]
    fn pull_uses_camel_case_from_seq() {
        // Katulong's handler keys on `fromSeq`, not `from_seq`.
        // The `#[serde(rename = "fromSeq")]` is load-bearing.
        let msg = Outbound::Pull {
            from_seq: 4096,
            session: Some("s".into()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"pull","fromSeq":4096,"session":"s"}"#);
    }

    #[test]
    fn ping_serializes_with_no_body() {
        let msg = Outbound::Ping;
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"ping"}"#);
    }

    #[test]
    fn resize_omits_session_when_unset() {
        let msg = Outbound::Resize {
            cols: 80,
            rows: 24,
            session: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"resize","cols":80,"rows":24}"#);
    }

    #[test]
    fn resync_serializes_as_minimal_object() {
        let msg = Outbound::Resync { session: None };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"resync"}"#);
    }

    // ── inbound decoding ────────────────────────────────────

    #[test]
    fn attached_decodes_with_serialized_buffer() {
        // The `data` payload from `Attached` is whatever
        // `session.snapshot()` produced — a serialized xterm
        // screen. We don't parse it here; we just hand it to
        // the rolling buffer. Verify deserialization is lossless:
        // a valid JSON string containing the escape `\u001b`
        // must round-trip back to a Rust string containing the
        // raw ESC byte.
        let raw = r#"{"type":"attached","session":"s","data":"hello\u001b[2J"}"#;
        let parsed: Inbound = serde_json::from_str(raw).unwrap();
        match parsed {
            Inbound::Attached { session, data } => {
                assert_eq!(session, "s");
                assert_eq!(data, "hello\u{001b}[2J");
            }
            other => panic!("expected Attached, got {other:?}"),
        }
    }

    #[test]
    fn seq_init_decodes_kebab_case_type() {
        let raw = r#"{"type":"seq-init","session":"s","seq":1234}"#;
        let parsed: Inbound = serde_json::from_str(raw).unwrap();
        assert_eq!(
            parsed,
            Inbound::SeqInit {
                session: "s".into(),
                seq: 1234,
            }
        );
    }

    #[test]
    fn pull_response_decodes_normal_path() {
        let raw = r#"{"type":"pull-response","session":"s","data":"more","cursor":4100}"#;
        let parsed: Inbound = serde_json::from_str(raw).unwrap();
        assert_eq!(
            parsed,
            Inbound::PullResponse {
                session: "s".into(),
                data: "more".into(),
                cursor: 4100,
            }
        );
    }

    #[test]
    fn pull_response_backpressure_path_has_empty_data() {
        // When katulong is backpressured (per ws-manager.js:437),
        // it returns the same shape but with empty `data` and an
        // advanced `cursor`. Sipag advances and pulls again.
        let raw = r#"{"type":"pull-response","session":"s","data":"","cursor":99999}"#;
        let parsed: Inbound = serde_json::from_str(raw).unwrap();
        match parsed {
            Inbound::PullResponse { data, cursor, .. } => {
                assert!(data.is_empty());
                assert_eq!(cursor, 99999);
            }
            other => panic!("expected PullResponse, got {other:?}"),
        }
    }

    #[test]
    fn pull_snapshot_decodes_eviction_recovery() {
        let raw = r#"{"type":"pull-snapshot","session":"s","data":"<snapshot>","cursor":12345}"#;
        let parsed: Inbound = serde_json::from_str(raw).unwrap();
        assert_eq!(
            parsed,
            Inbound::PullSnapshot {
                session: "s".into(),
                data: "<snapshot>".into(),
                cursor: 12345,
            }
        );
    }

    #[test]
    fn output_inline_push_includes_from_seq_and_cursor() {
        let raw = r#"{"type":"output","session":"s","data":"x","fromSeq":100,"cursor":101}"#;
        let parsed: Inbound = serde_json::from_str(raw).unwrap();
        assert_eq!(
            parsed,
            Inbound::Output {
                session: "s".into(),
                data: "x".into(),
                from_seq: 100,
                cursor: 101,
            }
        );
    }

    #[test]
    fn data_available_carries_no_data() {
        // Important property: `data-available` is a nudge only.
        // If sipag ever finds itself reading a `data` field from
        // it, something has gone wrong upstream.
        let raw = r#"{"type":"data-available","session":"s"}"#;
        let parsed: Inbound = serde_json::from_str(raw).unwrap();
        assert_eq!(
            parsed,
            Inbound::DataAvailable {
                session: "s".into()
            }
        );
    }

    #[test]
    fn state_check_decodes_integer_or_string_fingerprint() {
        // Katulong's ws-manager broadcasts DJB2 hashes as signed
        // 32-bit integers; older versions used hex strings. We
        // model `fingerprint` as `serde_json::Value` so anything
        // katulong might emit (now or later) parses cleanly; sipag
        // doesn't read the value. Pin the "accept anything" contract
        // with a spread of shapes so a regression to a stricter
        // type fails at test time.
        for raw in [
            r#"{"type":"state-check","session":"s","fingerprint":462302280,"seq":42}"#,
            r#"{"type":"state-check","session":"s","fingerprint":-1397367636,"seq":99}"#,
            r#"{"type":"state-check","session":"s","fingerprint":"abc","seq":17}"#,
            r#"{"type":"state-check","session":"s","fingerprint":null,"seq":1}"#,
            r#"{"type":"state-check","session":"s","fingerprint":3.14,"seq":2}"#,
            r#"{"type":"state-check","session":"s","fingerprint":true,"seq":3}"#,
            r#"{"type":"state-check","session":"s","fingerprint":[1,2,3],"seq":4}"#,
            r#"{"type":"state-check","session":"s","fingerprint":{"a":1},"seq":5}"#,
        ] {
            let parsed: Inbound = serde_json::from_str(raw).expect(raw);
            assert!(matches!(parsed, Inbound::StateCheck { .. }), "raw={raw}");
        }
    }

    #[test]
    fn exit_decodes_negative_codes() {
        // Per ws-manager.js:172, abnormal-death sessions report
        // `code: -1`. We model the field as i32 so it survives.
        let raw = r#"{"type":"exit","session":"s","code":-1}"#;
        let parsed: Inbound = serde_json::from_str(raw).unwrap();
        assert_eq!(
            parsed,
            Inbound::Exit {
                session: "s".into(),
                code: -1,
            }
        );
    }

    #[test]
    fn error_surfaces_message() {
        let raw = r#"{"type":"error","message":"Invalid session name"}"#;
        let parsed: Inbound = serde_json::from_str(raw).unwrap();
        assert_eq!(
            parsed,
            Inbound::Error {
                message: "Invalid session name".into(),
            }
        );
    }

    #[test]
    fn pong_decodes_with_no_body() {
        let raw = r#"{"type":"pong"}"#;
        let parsed: Inbound = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed, Inbound::Pong);
    }

    #[test]
    fn unknown_message_types_fall_through_to_other() {
        // This is the future-proofing test. Katulong adds a new
        // message type (e.g., `tab-icon-changed` was added after
        // this protocol's initial release). Sipag's read loop
        // sees `Other` and logs; it doesn't error out.
        let raw = r#"{"type":"tab-icon-changed","session":"s","icon":"robot"}"#;
        let parsed: Inbound = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed, Inbound::Other);

        // Same for completely unknown types — important so
        // sipag is forward-compatible with katulong evolutions.
        let raw = r#"{"type":"some-new-event","data":42}"#;
        let parsed: Inbound = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed, Inbound::Other);
    }

    #[test]
    fn session_updated_keeps_full_meta_value() {
        // session-updated carries the full session.toJSON() —
        // we don't model every meta field here, just keep it as
        // a serde_json::Value so callers can dig for what they
        // need (e.g., meta.claude.uuid first-appearance).
        let raw = r#"{"type":"session-updated","session":"s","data":{"id":"x","meta":{"claude":{"uuid":"u-1"}}}}"#;
        let parsed: Inbound = serde_json::from_str(raw).unwrap();
        match parsed {
            Inbound::SessionUpdated { session, data } => {
                assert_eq!(session, "s");
                assert_eq!(data["meta"]["claude"]["uuid"], "u-1");
            }
            other => panic!("expected SessionUpdated, got {other:?}"),
        }
    }

    #[test]
    fn session_removed_decodes() {
        // Load-bearing: drives `mark_terminal(SessionRemoved)` in
        // the dispatch handler. A deserialization regression would
        // silently route the message to `Other` and the attach
        // would never go terminal.
        let raw = r#"{"type":"session-removed","session":"s"}"#;
        let parsed: Inbound = serde_json::from_str(raw).unwrap();
        assert_eq!(
            parsed,
            Inbound::SessionRemoved {
                session: "s".into()
            }
        );
    }

    #[test]
    fn session_renamed_decodes() {
        let raw = r#"{"type":"session-renamed","name":"new-name","id":"sid-123"}"#;
        let parsed: Inbound = serde_json::from_str(raw).unwrap();
        assert_eq!(
            parsed,
            Inbound::SessionRenamed {
                name: "new-name".into(),
                id: "sid-123".into(),
            }
        );
    }

    #[test]
    fn switched_decodes() {
        let raw = r#"{"type":"switched","session":"s"}"#;
        let parsed: Inbound = serde_json::from_str(raw).unwrap();
        assert_eq!(
            parsed,
            Inbound::Switched {
                session: "s".into()
            }
        );
    }

    #[test]
    fn resize_sync_decodes() {
        let raw = r#"{"type":"resize-sync","cols":120,"rows":40}"#;
        let parsed: Inbound = serde_json::from_str(raw).unwrap();
        assert_eq!(
            parsed,
            Inbound::ResizeSync {
                cols: 120,
                rows: 40
            }
        );
    }
}
