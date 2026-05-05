//! File-backed durable pub/sub broker.
//!
//! Mirrors the on-disk layout used by `katulong/lib/topic-broker.js` so
//! external tools (and future katulong/sipag bridges) can `tail -f` the
//! log files of any topic without coupling to the broker's internals.
//!
//! Layout under `<sipag_dir>/pubsub/`:
//!
//! ```text
//! pubsub/
//!   <topic-segment>/
//!     ...nested.../
//!       log.jsonl   # append-only, one envelope per line
//!       seq         # last seq number, atomic-replace
//! ```
//!
//! Each envelope is a single-line JSON object:
//! `{seq, ts, topic, kind, payload}`. The `ts` field is RFC3339.
//!
//! Concurrency: per-topic in-memory `tokio::sync::broadcast` channels
//! handle live fanout. Publishes use a per-topic `Mutex` to keep seq
//! assignment + log append + seq write atomic from a single process'
//! point of view (multi-process publishers are still serialized by the
//! filesystem rename used for `seq`, but seq numbering cannot be
//! guaranteed monotonic across processes — that's a katulong-parity
//! tradeoff).

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::broadcast;

/// One message on a topic. The broker assigns `seq` and `ts`; callers
/// supply `topic`, `kind`, and `payload`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub seq: u64,
    pub ts: String,
    pub topic: String,
    pub kind: String,
    pub payload: serde_json::Value,
}

/// In-memory state for one topic: a broadcast sender (live fanout) and
/// a mutex protecting seq assignment + disk append.
struct TopicState {
    /// Lock guarding seq counter + log append. Held only briefly during
    /// publish.
    write_lock: StdMutex<u64>,
    /// Live broadcast channel. Subscribers receive envelopes published
    /// after they call `subscribe`.
    sender: broadcast::Sender<Envelope>,
}

/// File-backed pub/sub broker.
///
/// Cheap to clone (everything is behind `Arc` internally).
#[derive(Clone)]
pub struct Broker {
    inner: Arc<Inner>,
}

struct Inner {
    pubsub_dir: PathBuf,
    /// Topic name → state. Created lazily on first publish/subscribe.
    topics: StdMutex<HashMap<String, Arc<TopicState>>>,
}

impl Broker {
    /// Open (or create) a broker rooted at `<sipag_dir>/pubsub/`.
    pub fn open(sipag_dir: &Path) -> Result<Self> {
        let pubsub_dir = sipag_dir.join("pubsub");
        std::fs::create_dir_all(&pubsub_dir)
            .with_context(|| format!("create pubsub dir {}", pubsub_dir.display()))?;
        Ok(Self {
            inner: Arc::new(Inner {
                pubsub_dir,
                topics: StdMutex::new(HashMap::new()),
            }),
        })
    }

    /// Root directory for on-disk storage.
    pub fn pubsub_dir(&self) -> &Path {
        &self.inner.pubsub_dir
    }

    /// Resolve a topic to its directory, refusing path traversal.
    fn topic_dir(&self, topic: &str) -> Result<PathBuf> {
        if topic.is_empty() {
            return Err(anyhow!("topic must be non-empty"));
        }
        if topic.contains("..") || topic.starts_with('/') {
            return Err(anyhow!("invalid topic: {topic}"));
        }
        for seg in topic.split('/') {
            if seg.is_empty() || seg == "." || seg == ".." {
                return Err(anyhow!("invalid topic segment in {topic}"));
            }
            // Reject control chars and characters likely to be filesystem
            // hostile. Allow letters, digits, dash, underscore, dot.
            if !seg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            {
                return Err(anyhow!("topic segment has illegal characters: {seg}"));
            }
        }
        Ok(self.inner.pubsub_dir.join(topic))
    }

    /// Get or create the in-memory state for `topic`.
    fn state(&self, topic: &str) -> Arc<TopicState> {
        let mut topics = self
            .inner
            .topics
            .lock()
            .expect("pubsub topics lock poisoned");
        if let Some(s) = topics.get(topic) {
            return s.clone();
        }
        // Initialize seq from disk so restart-resume works.
        let dir = self.inner.pubsub_dir.join(topic);
        let seq = read_seq(&dir).unwrap_or(0);
        let (tx, _rx) = broadcast::channel::<Envelope>(1024);
        let state = Arc::new(TopicState {
            write_lock: StdMutex::new(seq),
            sender: tx,
        });
        topics.insert(topic.to_string(), state.clone());
        state
    }

    /// Publish to `topic`. Appends to `log.jsonl`, atomic-replaces
    /// `seq`, and broadcasts to live subscribers. Returns the assigned
    /// envelope.
    pub fn publish(&self, topic: &str, kind: &str, payload: serde_json::Value) -> Result<Envelope> {
        let dir = self.topic_dir(topic)?;
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("create topic dir {}", dir.display()))?;

        let state = self.state(topic);

        let mut seq_guard = state
            .write_lock
            .lock()
            .expect("pubsub topic write_lock poisoned");
        let next_seq = *seq_guard + 1;

        let envelope = Envelope {
            seq: next_seq,
            ts: chrono::Utc::now()
                .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                .to_string(),
            topic: topic.to_string(),
            kind: kind.to_string(),
            payload,
        };

        // Single-line append. POSIX guarantees writes <= PIPE_BUF (≥512)
        // are atomic on local FS; envelopes typically fit easily.
        let line = serde_json::to_string(&envelope).context("serialize envelope")?;
        append_line(&dir.join("log.jsonl"), &line).context("append envelope to log")?;

        // Atomic-replace `seq`.
        atomic_write_str(&dir.join("seq"), &next_seq.to_string()).context("write seq file")?;

        *seq_guard = next_seq;
        drop(seq_guard);

        // Broadcast to live subscribers; ignore SendError when nobody is
        // listening — durability is the on-disk log.
        let _ = state.sender.send(envelope.clone());
        Ok(envelope)
    }

    /// Subscribe to `topic`. Returns a `broadcast::Receiver` for new
    /// envelopes published after the subscription is established. This
    /// method does NOT replay history — use `read` to pull historical
    /// envelopes, then drain the receiver for live ones. Callers that
    /// need a single combined stream can interleave themselves.
    pub fn subscribe(&self, topic: &str) -> broadcast::Receiver<Envelope> {
        self.state(topic).sender.subscribe()
    }

    /// Pure historical read: envelopes from `topic` with `seq >= from_seq`.
    /// Returns an empty vec when the topic doesn't exist on disk yet.
    pub fn read(&self, topic: &str, from_seq: u64) -> Result<Vec<Envelope>> {
        let dir = self.topic_dir(topic)?;
        let log_path = dir.join("log.jsonl");
        if !log_path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(&log_path).with_context(|| format!("open {}", log_path.display()))?;
        let reader = BufReader::new(file);
        let mut out = Vec::new();
        for line in reader.lines() {
            let Ok(line) = line else { break };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<Envelope>(trimmed) {
                Ok(env) => {
                    if env.seq >= from_seq {
                        out.push(env);
                    }
                }
                Err(_) => continue, // skip malformed
            }
        }
        Ok(out)
    }

    /// List all topics that have on-disk state. Used by introspection.
    pub fn list_topics(&self) -> Result<Vec<String>> {
        let mut topics = Vec::new();
        walk_topics(&self.inner.pubsub_dir, "", &mut topics);
        topics.sort();
        Ok(topics)
    }
}

// ── helpers ─────────────────────────────────────────────────────────

fn walk_topics(base: &Path, prefix: &str, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    let base_path = base.to_path_buf();
    let has_seq = base_path.join("seq").is_file();
    if has_seq && !prefix.is_empty() {
        out.push(prefix.to_string());
    }
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let next_prefix = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            walk_topics(&path, &next_prefix, out);
        }
    }
}

fn read_seq(dir: &Path) -> Option<u64> {
    let path = dir.join("seq");
    let s = std::fs::read_to_string(&path).ok()?;
    s.trim().parse::<u64>().ok()
}

fn append_line(path: &Path, line: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open {} for append", path.display()))?;
    // Single write of "<line>\n" — POSIX ensures atomicity for short
    // writes within PIPE_BUF on the same file.
    let mut buf = String::with_capacity(line.len() + 1);
    buf.push_str(line);
    buf.push('\n');
    file.write_all(buf.as_bytes())
        .with_context(|| format!("append to {}", path.display()))?;
    Ok(())
}

fn atomic_write_str(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(content.as_bytes())?;
    tmp.flush()?;
    tmp.persist(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn open(dir: &TempDir) -> Broker {
        Broker::open(dir.path()).unwrap()
    }

    #[test]
    fn publish_and_read_round_trip() {
        let dir = TempDir::new().unwrap();
        let b = open(&dir);
        let e1 = b.publish("topic/one", "tick", json!({"n": 1})).unwrap();
        let e2 = b.publish("topic/one", "tick", json!({"n": 2})).unwrap();
        assert_eq!(e1.seq, 1);
        assert_eq!(e2.seq, 2);

        let envs = b.read("topic/one", 0).unwrap();
        assert_eq!(envs.len(), 2);
        assert_eq!(envs[0].seq, 1);
        assert_eq!(envs[0].kind, "tick");
        assert_eq!(envs[0].payload["n"], 1);
        assert_eq!(envs[1].seq, 2);
    }

    #[test]
    fn read_filters_by_from_seq() {
        let dir = TempDir::new().unwrap();
        let b = open(&dir);
        for n in 1..=5 {
            b.publish("t", "k", json!({"n": n})).unwrap();
        }
        let from3 = b.read("t", 3).unwrap();
        assert_eq!(from3.len(), 3);
        assert_eq!(from3.first().unwrap().seq, 3);
    }

    #[test]
    fn restart_resumes_seq() {
        let dir = TempDir::new().unwrap();
        {
            let b = open(&dir);
            b.publish("hello", "k", json!({})).unwrap();
            b.publish("hello", "k", json!({})).unwrap();
        }
        // Reopen and publish again — seq should continue at 3.
        let b = open(&dir);
        let e3 = b.publish("hello", "k", json!({})).unwrap();
        assert_eq!(e3.seq, 3);
        let envs = b.read("hello", 0).unwrap();
        assert_eq!(envs.len(), 3);
        assert_eq!(envs.last().unwrap().seq, 3);
    }

    #[tokio::test]
    async fn live_subscribers_receive_envelopes() {
        let dir = TempDir::new().unwrap();
        let b = open(&dir);
        let mut rx = b.subscribe("live/topic");
        let env = b.publish("live/topic", "msg", json!({"x": 1})).unwrap();
        let received = rx.recv().await.unwrap();
        assert_eq!(received.seq, env.seq);
        assert_eq!(received.kind, "msg");
        assert_eq!(received.payload["x"], 1);
    }

    #[test]
    fn read_unknown_topic_is_empty() {
        let dir = TempDir::new().unwrap();
        let b = open(&dir);
        assert!(b.read("nope", 0).unwrap().is_empty());
    }

    #[test]
    fn invalid_topic_rejected() {
        let dir = TempDir::new().unwrap();
        let b = open(&dir);
        assert!(b.publish("../escape", "k", json!({})).is_err());
        assert!(b.publish("/abs", "k", json!({})).is_err());
        assert!(b.publish("a//b", "k", json!({})).is_err());
        assert!(b.publish("", "k", json!({})).is_err());
        assert!(b.publish("a/b\u{0}c", "k", json!({})).is_err());
    }

    #[test]
    fn list_topics_after_publish() {
        let dir = TempDir::new().unwrap();
        let b = open(&dir);
        b.publish("a/b", "k", json!({})).unwrap();
        b.publish("c", "k", json!({})).unwrap();
        let mut topics = b.list_topics().unwrap();
        topics.sort();
        assert_eq!(topics, vec!["a/b".to_string(), "c".to_string()]);
    }

    #[tokio::test]
    async fn concurrent_publishers_assign_unique_seqs() {
        let dir = TempDir::new().unwrap();
        let b = open(&dir);

        let mut handles = Vec::new();
        for _ in 0..8 {
            let bc = b.clone();
            handles.push(tokio::task::spawn_blocking(move || {
                let mut seqs = Vec::new();
                for i in 0..25 {
                    let e = bc.publish("conc", "k", json!({"i": i})).unwrap();
                    seqs.push(e.seq);
                }
                seqs
            }));
        }

        let mut all = Vec::new();
        for h in handles {
            all.extend(h.await.unwrap());
        }
        all.sort();
        // 8 * 25 = 200 publishes — all seqs must be unique 1..=200.
        assert_eq!(all.len(), 200);
        for (i, seq) in all.iter().enumerate() {
            assert_eq!(*seq as usize, i + 1);
        }

        let envs = b.read("conc", 0).unwrap();
        assert_eq!(envs.len(), 200);
    }

    #[test]
    fn on_disk_layout_matches_expected() {
        let dir = TempDir::new().unwrap();
        let b = open(&dir);
        b.publish(
            "workers/activity",
            "worker.start",
            json!({"name": "research"}),
        )
        .unwrap();
        let topic_dir = dir.path().join("pubsub/workers/activity");
        assert!(topic_dir.join("log.jsonl").exists());
        assert!(topic_dir.join("seq").exists());
        let seq = std::fs::read_to_string(topic_dir.join("seq")).unwrap();
        assert_eq!(seq.trim(), "1");
        let log = std::fs::read_to_string(topic_dir.join("log.jsonl")).unwrap();
        let env: Envelope = serde_json::from_str(log.trim()).unwrap();
        assert_eq!(env.seq, 1);
        assert_eq!(env.topic, "workers/activity");
        assert_eq!(env.kind, "worker.start");
    }
}
