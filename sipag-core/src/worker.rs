use anyhow::Result;
use std::{fs, path::Path, path::PathBuf};

/// Worker state parsed from `~/.sipag/workers/*.json`.
///
/// Written by `lib/worker/docker.sh` when a worker starts and updated on completion.
#[derive(Debug, Clone)]
pub struct WorkerState {
    pub repo: String,
    pub issue_num: u64,
    pub issue_title: String,
    pub branch: String,
    pub container_name: String,
    pub pr_num: Option<u64>,
    pub pr_url: Option<String>,
    /// One of "running", "done", or "failed".
    pub status: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub duration_s: Option<i64>,
    pub exit_code: Option<i64>,
    pub log_path: Option<PathBuf>,
}

/// Read all worker state files from `<sipag_dir>/workers/*.json`.
///
/// Files are sorted by name (OWNER--REPO--ISSUE_NUM.json) for stable ordering.
/// Malformed JSON files are silently skipped.
pub fn list_workers(sipag_dir: &Path) -> Result<Vec<WorkerState>> {
    let workers_dir = sipag_dir.join("workers");
    if !workers_dir.exists() {
        return Ok(vec![]);
    }

    let mut paths: Vec<PathBuf> = fs::read_dir(&workers_dir)?
        .flatten()
        .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
        .map(|e| e.path())
        .collect();
    paths.sort();

    let mut workers = vec![];
    for path in paths {
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(state) = parse_worker_state(&content) {
                workers.push(state);
            }
        }
    }

    Ok(workers)
}

fn parse_worker_state(json: &str) -> Result<WorkerState> {
    let v: serde_json::Value = serde_json::from_str(json)?;
    Ok(WorkerState {
        repo: v["repo"].as_str().unwrap_or("").to_string(),
        issue_num: v["issue_num"].as_u64().unwrap_or(0),
        issue_title: v["issue_title"].as_str().unwrap_or("").to_string(),
        branch: v["branch"].as_str().unwrap_or("").to_string(),
        container_name: v["container_name"].as_str().unwrap_or("").to_string(),
        pr_num: v["pr_num"].as_u64(),
        pr_url: v["pr_url"].as_str().map(|s| s.to_string()),
        status: v["status"].as_str().unwrap_or("").to_string(),
        started_at: v["started_at"].as_str().map(|s| s.to_string()),
        ended_at: v["ended_at"].as_str().map(|s| s.to_string()),
        duration_s: v["duration_s"].as_i64(),
        exit_code: v["exit_code"].as_i64(),
        log_path: v["log_path"].as_str().map(PathBuf::from),
    })
}

/// Format a duration in seconds to a human-readable string like "4m23s".
pub fn format_worker_duration(duration_s: Option<i64>) -> String {
    match duration_s {
        Some(s) if s >= 0 => format!("{}m{}s", s / 60, s % 60),
        _ => "-".to_string(),
    }
}

/// Return the display string for the branch/PR column.
///
/// Done workers with a PR show "PR #N"; others show the branch name.
pub fn branch_display(worker: &WorkerState) -> String {
    if worker.status == "done" {
        if let Some(pr_num) = worker.pr_num {
            return format!("PR #{}", pr_num);
        }
    }
    worker.branch.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn sample_json(status: &str, duration_s: Option<i64>, pr_num: Option<u64>) -> String {
        let dur = duration_s
            .map(|d| d.to_string())
            .unwrap_or_else(|| "null".to_string());
        let pr = pr_num
            .map(|n| n.to_string())
            .unwrap_or_else(|| "null".to_string());
        format!(
            r#"{{
                "repo": "Dorky-Robot/sipag",
                "issue_num": 42,
                "issue_title": "Fix the thing",
                "branch": "sipag/issue-42-fix-the-thing",
                "container_name": "sipag-issue-42",
                "pr_num": {pr},
                "pr_url": null,
                "status": "{status}",
                "started_at": "2024-01-15T10:30:00Z",
                "ended_at": null,
                "duration_s": {dur},
                "exit_code": null,
                "log_path": "/home/.sipag/logs/Dorky-Robot--sipag--42.log"
            }}"#
        )
    }

    #[test]
    fn parse_running_worker() {
        let json = sample_json("running", None, None);
        let w = parse_worker_state(&json).unwrap();
        assert_eq!(w.repo, "Dorky-Robot/sipag");
        assert_eq!(w.issue_num, 42);
        assert_eq!(w.status, "running");
        assert_eq!(w.pr_num, None);
        assert_eq!(w.duration_s, None);
    }

    #[test]
    fn parse_done_worker() {
        let json = sample_json("done", Some(263), Some(163));
        let w = parse_worker_state(&json).unwrap();
        assert_eq!(w.status, "done");
        assert_eq!(w.pr_num, Some(163));
        assert_eq!(w.duration_s, Some(263));
    }

    #[test]
    fn format_duration_variants() {
        assert_eq!(format_worker_duration(None), "-");
        assert_eq!(format_worker_duration(Some(0)), "0m0s");
        assert_eq!(format_worker_duration(Some(263)), "4m23s");
        assert_eq!(format_worker_duration(Some(3600)), "60m0s");
    }

    #[test]
    fn branch_display_running() {
        let json = sample_json("running", None, None);
        let w = parse_worker_state(&json).unwrap();
        assert_eq!(branch_display(&w), "sipag/issue-42-fix-the-thing");
    }

    #[test]
    fn branch_display_done_with_pr() {
        let json = sample_json("done", Some(300), Some(163));
        let w = parse_worker_state(&json).unwrap();
        assert_eq!(branch_display(&w), "PR #163");
    }

    #[test]
    fn list_workers_missing_dir() {
        let dir = TempDir::new().unwrap();
        let workers = list_workers(dir.path()).unwrap();
        assert!(workers.is_empty());
    }

    #[test]
    fn list_workers_reads_json_files() {
        let dir = TempDir::new().unwrap();
        let workers_dir = dir.path().join("workers");
        fs::create_dir(&workers_dir).unwrap();

        let mut f = fs::File::create(workers_dir.join("Dorky-Robot--sipag--42.json")).unwrap();
        writeln!(f, "{}", sample_json("running", None, None)).unwrap();

        let workers = list_workers(dir.path()).unwrap();
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].issue_num, 42);
    }

    #[test]
    fn list_workers_skips_invalid_json() {
        let dir = TempDir::new().unwrap();
        let workers_dir = dir.path().join("workers");
        fs::create_dir(&workers_dir).unwrap();

        fs::write(workers_dir.join("bad.json"), "not json").unwrap();
        let mut f = fs::File::create(workers_dir.join("good.json")).unwrap();
        writeln!(f, "{}", sample_json("done", Some(120), Some(99))).unwrap();

        let workers = list_workers(dir.path()).unwrap();
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].status, "done");
    }
}
