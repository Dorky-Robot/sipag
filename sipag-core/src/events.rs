/// Structured progress events for worker tasks (NDJSON format).
///
/// Events are appended to `~/.sipag/running/<task-id>.events` as newline-delimited
/// JSON. The TUI and `sipag logs --follow` consume this file for real-time progress.
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

/// Known event types emitted during task execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventType {
    Started,
    Cloning,
    Planning,
    Coding,
    Testing,
    Committing,
    PrOpened,
    Done,
    Failed,
}

impl EventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            EventType::Started => "started",
            EventType::Cloning => "cloning",
            EventType::Planning => "planning",
            EventType::Coding => "coding",
            EventType::Testing => "testing",
            EventType::Committing => "committing",
            EventType::PrOpened => "pr_opened",
            EventType::Done => "done",
            EventType::Failed => "failed",
        }
    }

    /// Parse an event type from its string representation.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "started" => Some(EventType::Started),
            "cloning" => Some(EventType::Cloning),
            "planning" => Some(EventType::Planning),
            "coding" => Some(EventType::Coding),
            "testing" => Some(EventType::Testing),
            "committing" => Some(EventType::Committing),
            "pr_opened" => Some(EventType::PrOpened),
            "done" => Some(EventType::Done),
            "failed" => Some(EventType::Failed),
            _ => None,
        }
    }
}

/// A single progress event.
#[derive(Debug, Clone)]
pub struct WorkerEvent {
    pub ts: String,
    pub event: String,
    pub issue: Option<u32>,
    pub msg: String,
}

impl WorkerEvent {
    /// Serialize to a single NDJSON line (no trailing newline).
    pub fn to_ndjson(&self) -> String {
        let issue_part = match self.issue {
            Some(n) => format!(r#","issue":{n}"#),
            None => String::new(),
        };
        // Escape msg for JSON (handle backslash, quotes, and control chars)
        let escaped_msg = escape_json_string(&self.msg);
        format!(
            r#"{{"ts":"{ts}","event":"{event}"{issue_part},"msg":"{msg}"}}"#,
            ts = self.ts,
            event = self.event,
            issue_part = issue_part,
            msg = escaped_msg,
        )
    }

    /// Try to parse a NDJSON line into a WorkerEvent. Returns None if parsing fails.
    pub fn from_ndjson(line: &str) -> Option<Self> {
        let line = line.trim();
        if line.is_empty() || !line.starts_with('{') {
            return None;
        }

        let ts = extract_json_string(line, "ts")?;
        let event = extract_json_string(line, "event")?;
        let msg = extract_json_string(line, "msg").unwrap_or_default();
        let issue = extract_json_u32(line, "issue");

        Some(WorkerEvent { ts, event, issue, msg })
    }
}

/// Append a single event to the events file at `events_path`.
/// Creates the file if it does not exist. Silently ignores I/O errors so that
/// event emission never crashes the main execution path.
pub fn emit_event(events_path: &Path, event_type: EventType, issue: Option<u32>, msg: &str) {
    let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let ev = WorkerEvent {
        ts,
        event: event_type.as_str().to_string(),
        issue,
        msg: msg.to_string(),
    };
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(events_path) {
        let _ = writeln!(file, "{}", ev.to_ndjson());
    }
}

/// Read all events from an events file. Returns an empty vec on any error.
pub fn read_events(events_path: &Path) -> Vec<WorkerEvent> {
    let Ok(content) = std::fs::read_to_string(events_path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(WorkerEvent::from_ndjson)
        .collect()
}

/// Read the last event from an events file. Returns None if file is missing or empty.
pub fn last_event(events_path: &Path) -> Option<WorkerEvent> {
    let content = std::fs::read_to_string(events_path).ok()?;
    content.lines().filter_map(WorkerEvent::from_ndjson).last()
}

// ── Minimal JSON helpers (no external deps) ───────────────────────────────────

/// Escape a string for embedding in a JSON value.
fn escape_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// Very small JSON string extractor: finds `"key":"value"` in a flat JSON object.
/// Not a full parser — only handles the simple NDJSON format we emit.
fn extract_json_string(json: &str, key: &str) -> Option<String> {
    let needle = format!(r#""{key}":""#);
    let start = json.find(&needle)? + needle.len();
    let rest = &json[start..];
    let mut out = String::new();
    let mut chars = rest.chars();
    loop {
        match chars.next()? {
            '"' => break,
            '\\' => match chars.next()? {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                c => {
                    out.push('\\');
                    out.push(c);
                }
            },
            c => out.push(c),
        }
    }
    Some(out)
}

/// Extract a u32 value from `"key":N` in a flat JSON object.
fn extract_json_u32(json: &str, key: &str) -> Option<u32> {
    let needle = format!(r#""{key}":"#);
    let start = json.find(&needle)? + needle.len();
    let rest = &json[start..];
    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_event_type_roundtrip() {
        for et in &[
            EventType::Started,
            EventType::Cloning,
            EventType::Planning,
            EventType::Coding,
            EventType::Testing,
            EventType::Committing,
            EventType::PrOpened,
            EventType::Done,
            EventType::Failed,
        ] {
            let s = et.as_str();
            assert_eq!(EventType::from_str(s).as_ref(), Some(et));
        }
    }

    #[test]
    fn test_worker_event_to_ndjson() {
        let ev = WorkerEvent {
            ts: "2026-02-20T18:00:00Z".to_string(),
            event: "coding".to_string(),
            issue: Some(42),
            msg: "Implementing auth middleware".to_string(),
        };
        let json = ev.to_ndjson();
        assert!(json.contains(r#""ts":"2026-02-20T18:00:00Z""#));
        assert!(json.contains(r#""event":"coding""#));
        assert!(json.contains(r#""issue":42"#));
        assert!(json.contains(r#""msg":"Implementing auth middleware""#));
    }

    #[test]
    fn test_worker_event_to_ndjson_no_issue() {
        let ev = WorkerEvent {
            ts: "2026-02-20T18:00:00Z".to_string(),
            event: "started".to_string(),
            issue: None,
            msg: "Task started".to_string(),
        };
        let json = ev.to_ndjson();
        assert!(!json.contains("issue"));
        assert!(json.contains(r#""event":"started""#));
    }

    #[test]
    fn test_worker_event_roundtrip() {
        let ev = WorkerEvent {
            ts: "2026-02-20T18:00:00Z".to_string(),
            event: "testing".to_string(),
            issue: Some(7),
            msg: "Running cargo test".to_string(),
        };
        let json = ev.to_ndjson();
        let parsed = WorkerEvent::from_ndjson(&json).unwrap();
        assert_eq!(parsed.ts, ev.ts);
        assert_eq!(parsed.event, ev.event);
        assert_eq!(parsed.issue, ev.issue);
        assert_eq!(parsed.msg, ev.msg);
    }

    #[test]
    fn test_worker_event_roundtrip_no_issue() {
        let ev = WorkerEvent {
            ts: "2026-02-20T18:00:00Z".to_string(),
            event: "done".to_string(),
            issue: None,
            msg: "All done".to_string(),
        };
        let json = ev.to_ndjson();
        let parsed = WorkerEvent::from_ndjson(&json).unwrap();
        assert_eq!(parsed.event, "done");
        assert_eq!(parsed.issue, None);
    }

    #[test]
    fn test_emit_and_read_events() {
        let dir = TempDir::new().unwrap();
        let events_path = dir.path().join("task.events");

        emit_event(&events_path, EventType::Started, Some(42), "Task started");
        emit_event(&events_path, EventType::Coding, Some(42), "Writing code");
        emit_event(&events_path, EventType::Done, Some(42), "All done");

        let events = read_events(&events_path);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].event, "started");
        assert_eq!(events[1].event, "coding");
        assert_eq!(events[2].event, "done");
        assert_eq!(events[0].issue, Some(42));
    }

    #[test]
    fn test_last_event() {
        let dir = TempDir::new().unwrap();
        let events_path = dir.path().join("task.events");

        emit_event(&events_path, EventType::Started, None, "Started");
        emit_event(&events_path, EventType::Coding, None, "Coding");

        let last = last_event(&events_path).unwrap();
        assert_eq!(last.event, "coding");
    }

    #[test]
    fn test_last_event_missing_file() {
        let dir = TempDir::new().unwrap();
        let events_path = dir.path().join("nonexistent.events");
        assert!(last_event(&events_path).is_none());
    }

    #[test]
    fn test_msg_with_special_chars() {
        let dir = TempDir::new().unwrap();
        let events_path = dir.path().join("task.events");
        let msg = r#"Running "cargo test" with flag\path"#;
        emit_event(&events_path, EventType::Testing, None, msg);
        let events = read_events(&events_path);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].msg, msg);
    }

    #[test]
    fn test_from_ndjson_invalid() {
        assert!(WorkerEvent::from_ndjson("").is_none());
        assert!(WorkerEvent::from_ndjson("not json").is_none());
        assert!(WorkerEvent::from_ndjson("{}").is_none()); // missing ts
    }
}
