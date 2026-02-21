use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

/// State for a single `sipag work` issue worker, persisted as JSON in
/// `~/.sipag/workers/OWNER--REPO--N.json`.
///
/// Status transitions: `running` → `done` or `failed`.
/// The file is written on start and updated on completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerState {
    pub repo: String,
    pub issue_num: u64,
    pub issue_title: String,
    pub branch: String,
    pub pr_num: Option<u64>,
    pub pr_url: Option<String>,
    /// One of: "running", "done", "failed"
    pub status: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub duration_s: Option<f64>,
    pub exit_code: Option<i32>,
    /// Absolute path to the log file (may contain `~/` prefix)
    pub log_path: String,
}

impl WorkerState {
    /// Return a human-readable duration string.
    ///
    /// Uses `duration_s` for completed workers; computes elapsed time from
    /// `started_at` for running workers.
    pub fn format_duration(&self) -> String {
        if let Some(secs) = self.duration_s {
            return format_secs(secs as i64);
        }
        use chrono::DateTime;
        let started = DateTime::parse_from_rfc3339(&self.started_at)
            .ok()
            .map(|dt| dt.with_timezone(&chrono::Utc));
        match started {
            None => "-".to_string(),
            Some(start) => {
                let secs = (chrono::Utc::now() - start).num_seconds();
                format_secs(secs)
            }
        }
    }

    /// Resolve the log path, expanding a leading `~/` to the home directory.
    pub fn resolved_log_path(&self) -> std::path::PathBuf {
        if self.log_path.starts_with("~/") {
            let home = std::env::var("HOME").unwrap_or_default();
            std::path::PathBuf::from(format!("{}{}", home, &self.log_path[1..]))
        } else {
            std::path::PathBuf::from(&self.log_path)
        }
    }
}

/// Read all worker state JSON files from `sipag_dir/workers/`.
///
/// Files that fail to parse are skipped with a warning on stderr.
/// Results are sorted: running first, then done, then failed; within
/// each group ordered by `started_at` ascending.
pub fn list_workers(sipag_dir: &Path) -> Result<Vec<WorkerState>> {
    let workers_dir = sipag_dir.join("workers");
    if !workers_dir.exists() {
        return Ok(Vec::new());
    }

    let mut paths: Vec<_> = fs::read_dir(&workers_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .map(|e| e.path())
        .collect();
    paths.sort();

    let mut workers = Vec::new();
    for path in paths {
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Warning: could not read {:?}: {}", path, e);
                continue;
            }
        };
        match serde_json::from_str::<WorkerState>(&content) {
            Ok(w) => workers.push(w),
            Err(e) => eprintln!("Warning: failed to parse {:?}: {}", path, e),
        }
    }

    // Sort: running → done → failed; within each group by started_at asc
    workers.sort_by(|a, b| {
        let order = |s: &str| match s {
            "running" => 0u8,
            "done" => 1,
            "failed" => 2,
            _ => 3,
        };
        order(&a.status)
            .cmp(&order(&b.status))
            .then(a.started_at.cmp(&b.started_at))
    });

    Ok(workers)
}

fn format_secs(secs: i64) -> String {
    if secs < 0 {
        return "0s".to_string();
    }
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn make_worker(status: &str, started_at: &str) -> WorkerState {
        WorkerState {
            repo: "Owner/repo".to_string(),
            issue_num: 1,
            issue_title: "Test".to_string(),
            branch: "feat/test".to_string(),
            pr_num: None,
            pr_url: None,
            status: status.to_string(),
            started_at: started_at.to_string(),
            ended_at: None,
            duration_s: None,
            exit_code: None,
            log_path: "/tmp/test.log".to_string(),
        }
    }

    #[test]
    fn test_list_workers_empty_dir() {
        let dir = TempDir::new().unwrap();
        let workers_dir = dir.path().join("workers");
        std::fs::create_dir_all(&workers_dir).unwrap();
        let result = list_workers(dir.path()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_list_workers_missing_dir() {
        let dir = TempDir::new().unwrap();
        let result = list_workers(dir.path()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_list_workers_parses_json() {
        let dir = TempDir::new().unwrap();
        let workers_dir = dir.path().join("workers");
        std::fs::create_dir_all(&workers_dir).unwrap();

        let json = r#"{
            "repo": "Dorky-Robot/sipag",
            "issue_num": 148,
            "issue_title": "Split worker.sh",
            "branch": "sipag/issue-148-split",
            "pr_num": null,
            "pr_url": null,
            "status": "running",
            "started_at": "2026-02-20T20:21:00Z",
            "ended_at": null,
            "duration_s": null,
            "exit_code": null,
            "log_path": "/tmp/test.log"
        }"#;

        let mut f = fs::File::create(workers_dir.join("Dorky-Robot--sipag--148.json")).unwrap();
        write!(f, "{json}").unwrap();

        let workers = list_workers(dir.path()).unwrap();
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].repo, "Dorky-Robot/sipag");
        assert_eq!(workers[0].issue_num, 148);
        assert_eq!(workers[0].status, "running");
    }

    #[test]
    fn test_list_workers_sort_order() {
        let dir = TempDir::new().unwrap();
        let workers_dir = dir.path().join("workers");
        std::fs::create_dir_all(&workers_dir).unwrap();

        let states = vec![
            ("failed", "2026-02-20T10:00:00Z", "a.json"),
            ("done", "2026-02-20T09:00:00Z", "b.json"),
            ("running", "2026-02-20T11:00:00Z", "c.json"),
        ];

        for (status, started_at, fname) in &states {
            let w = make_worker(status, started_at);
            let json = serde_json::to_string(&w).unwrap();
            let mut f = fs::File::create(workers_dir.join(fname)).unwrap();
            write!(f, "{json}").unwrap();
        }

        let workers = list_workers(dir.path()).unwrap();
        assert_eq!(workers.len(), 3);
        assert_eq!(workers[0].status, "running");
        assert_eq!(workers[1].status, "done");
        assert_eq!(workers[2].status, "failed");
    }

    #[test]
    fn test_format_secs() {
        assert_eq!(format_secs(0), "0s");
        assert_eq!(format_secs(45), "45s");
        assert_eq!(format_secs(90), "1m30s");
        assert_eq!(format_secs(3661), "1h1m");
        assert_eq!(format_secs(-1), "0s");
    }

    #[test]
    fn test_format_duration_with_duration_s() {
        let mut w = make_worker("done", "2026-02-20T10:00:00Z");
        w.duration_s = Some(123.0);
        assert_eq!(w.format_duration(), "2m3s");
    }

    #[test]
    fn test_resolved_log_path_tilde() {
        let w = make_worker("running", "2026-02-20T10:00:00Z");
        let mut w2 = w.clone();
        w2.log_path = "~/.sipag/logs/test.log".to_string();
        let resolved = w2.resolved_log_path();
        // Should expand ~ to HOME
        assert!(!resolved.to_string_lossy().starts_with('~'));
    }

    #[test]
    fn test_resolved_log_path_absolute() {
        let mut w = make_worker("running", "2026-02-20T10:00:00Z");
        w.log_path = "/tmp/foo.log".to_string();
        let resolved = w.resolved_log_path();
        assert_eq!(resolved, std::path::PathBuf::from("/tmp/foo.log"));
    }
}
