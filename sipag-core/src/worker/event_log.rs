//! Structured JSONL event log for `sipag work`.
//!
//! Writes one JSON object per line to `~/.sipag/logs/worker.log`, making
//! progress observable via `tail -f ~/.sipag/logs/worker.log` without
//! duplicating the existing stdout output.
//!
//! ## Event types
//!
//! | `event`              | When                                                 |
//! |----------------------|------------------------------------------------------|
//! | `cycle_start`        | Poll loop iteration begins for a repo                |
//! | `cycle_end`          | Poll loop iteration ends for a repo                  |
//! | `issue_dispatch`     | Worker container is about to be started              |
//! | `worker_result`      | Worker container has exited                          |
//! | `issue_skipped`      | Issue excluded from dispatch (existing PR / label)   |
//! | `back_pressure`      | Dispatch paused due to open PR threshold             |
//! | `error`              | Non-fatal error during the poll cycle                |
//!
//! ## Format
//!
//! ```json
//! {"ts":"2026-02-22T10:00:00Z","event":"cycle_start","repo":"owner/repo"}
//! {"ts":"2026-02-22T10:00:01Z","event":"issue_dispatch","repo":"owner/repo","issues":[344,345],"container":"sipag-group-344","grouped":true}
//! {"ts":"2026-02-22T10:05:00Z","event":"worker_result","repo":"owner/repo","issues":[344,345],"success":true,"duration_s":299}
//! ```

use std::fs::OpenOptions;
use std::io::Write as IoWrite;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// Handle to the structured worker event log.
///
/// Created once per `run_worker_loop` invocation and passed to helpers that
/// emit events. Writes are best-effort — errors are silently ignored so that
/// a broken log path never disrupts the main worker loop.
pub struct EventLog {
    path: PathBuf,
}

impl EventLog {
    /// Open (or create) the worker log at `logs_dir/worker.log`.
    pub fn open(logs_dir: &Path) -> Self {
        Self {
            path: logs_dir.join("worker.log"),
        }
    }

    /// Append a JSON event object to the log file (one line per event).
    ///
    /// The `ts` field (ISO-8601 UTC timestamp) is injected automatically.
    pub fn emit(&self, mut event: Value) {
        if let Some(obj) = event.as_object_mut() {
            obj.insert(
                "ts".to_string(),
                Value::String(chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()),
            );
        }
        let mut line = event.to_string();
        line.push('\n');
        // Best-effort write — never panic on log failure.
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = f.write_all(line.as_bytes());
        }
    }

    // ── Typed helpers ────────────────────────────────────────────────────────

    pub fn cycle_start(&self, repo: &str) {
        self.emit(json!({"event": "cycle_start", "repo": repo}));
    }

    pub fn cycle_end(&self, repo: &str) {
        self.emit(json!({"event": "cycle_end", "repo": repo}));
    }

    /// Emit an `issue_dispatch` event before a worker container starts.
    pub fn issue_dispatch(&self, repo: &str, issues: &[u64], container: &str, grouped: bool) {
        self.emit(json!({
            "event": "issue_dispatch",
            "repo": repo,
            "issues": issues,
            "container": container,
            "grouped": grouped,
        }));
    }

    /// Emit a `worker_result` event after a worker container exits.
    pub fn worker_result(
        &self,
        repo: &str,
        issues: &[u64],
        success: bool,
        duration_s: u64,
        pr_num: Option<u64>,
        pr_url: Option<&str>,
    ) {
        let mut ev = json!({
            "event": "worker_result",
            "repo": repo,
            "issues": issues,
            "success": success,
            "duration_s": duration_s,
        });
        if let Some(n) = pr_num {
            ev["pr_num"] = json!(n);
        }
        if let Some(u) = pr_url {
            ev["pr_url"] = json!(u);
        }
        self.emit(ev);
    }

    /// Emit an `issue_skipped` event when an issue is excluded from dispatch.
    pub fn issue_skipped(&self, repo: &str, issue_num: u64, reason: &str, pr_num: Option<u64>) {
        let mut ev = json!({
            "event": "issue_skipped",
            "repo": repo,
            "issue": issue_num,
            "reason": reason,
        });
        if let Some(n) = pr_num {
            ev["pr_num"] = json!(n);
        }
        self.emit(ev);
    }

    /// Emit a `back_pressure` event when dispatch is paused.
    pub fn back_pressure(&self, repo: &str, open_prs: usize, threshold: usize) {
        self.emit(json!({
            "event": "back_pressure",
            "repo": repo,
            "open_prs": open_prs,
            "threshold": threshold,
        }));
    }

    /// Emit an `error` event for non-fatal errors.
    pub fn error(&self, repo: &str, message: &str) {
        self.emit(json!({
            "event": "error",
            "repo": repo,
            "message": message,
        }));
    }
}
