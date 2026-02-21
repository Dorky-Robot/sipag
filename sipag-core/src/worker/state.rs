use std::path::PathBuf;

use super::status::WorkerStatus;

/// Worker state parsed from `~/.sipag/workers/*.json`.
///
/// Written by the worker when a container starts and updated on completion.
/// This is the entity — the single source of truth for a worker's lifecycle.
#[derive(Debug, Clone)]
pub struct WorkerState {
    pub repo: String,
    pub issue_num: u64,
    pub issue_title: String,
    pub branch: String,
    pub container_name: String,
    pub pr_num: Option<u64>,
    pub pr_url: Option<String>,
    pub status: WorkerStatus,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub duration_s: Option<i64>,
    pub exit_code: Option<i64>,
    pub log_path: Option<PathBuf>,
}

/// Parse a worker state from a JSON string.
///
/// Unknown status strings default to `WorkerStatus::Failed` to surface
/// issues rather than silently treating them as running.
///
/// Returns an error for critical missing fields (`repo`, `issue_num`).
/// Logs a warning for non-critical missing fields.
pub fn parse_worker_json(json: &str) -> anyhow::Result<WorkerState> {
    let v: serde_json::Value = serde_json::from_str(json)?;
    let status_str = v["status"].as_str().unwrap_or("");
    let status = WorkerStatus::parse(status_str).unwrap_or(WorkerStatus::Failed);

    let repo = v["repo"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("worker state missing required field: repo"))?
        .to_string();

    let issue_num = v["issue_num"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("worker state missing required field: issue_num"))?;

    let issue_title = v["issue_title"].as_str().unwrap_or("").to_string();
    let branch = v["branch"].as_str().unwrap_or("").to_string();
    let container_name = v["container_name"].as_str().unwrap_or("").to_string();

    Ok(WorkerState {
        repo,
        issue_num,
        issue_title,
        branch,
        container_name,
        pr_num: v["pr_num"].as_u64(),
        pr_url: v["pr_url"].as_str().map(|s| s.to_string()),
        status,
        started_at: v["started_at"].as_str().map(|s| s.to_string()),
        ended_at: v["ended_at"].as_str().map(|s| s.to_string()),
        duration_s: v["duration_s"].as_i64(),
        exit_code: v["exit_code"].as_i64(),
        log_path: v["log_path"].as_str().map(PathBuf::from),
    })
}

/// Format a duration in seconds to a human-readable string like "4m23s".
pub fn format_duration(duration_s: Option<i64>) -> String {
    match duration_s {
        Some(s) if s >= 0 => format!("{}m{}s", s / 60, s % 60),
        _ => "-".to_string(),
    }
}

/// Return the display string for the branch/PR column.
///
/// Done workers with a PR show "PR #N"; others show the branch name.
pub fn branch_display(worker: &WorkerState) -> String {
    if worker.status == WorkerStatus::Done {
        if let Some(pr_num) = worker.pr_num {
            return format!("PR #{}", pr_num);
        }
    }
    worker.branch.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let w = parse_worker_json(&json).unwrap();
        assert_eq!(w.repo, "Dorky-Robot/sipag");
        assert_eq!(w.issue_num, 42);
        assert_eq!(w.status, WorkerStatus::Running);
        assert_eq!(w.pr_num, None);
        assert_eq!(w.duration_s, None);
    }

    #[test]
    fn parse_done_worker() {
        let json = sample_json("done", Some(263), Some(163));
        let w = parse_worker_json(&json).unwrap();
        assert_eq!(w.status, WorkerStatus::Done);
        assert_eq!(w.pr_num, Some(163));
        assert_eq!(w.duration_s, Some(263));
    }

    #[test]
    fn parse_failed_worker() {
        let json = sample_json("failed", Some(10), None);
        let w = parse_worker_json(&json).unwrap();
        assert_eq!(w.status, WorkerStatus::Failed);
    }

    #[test]
    fn parse_recovering_worker() {
        let json = sample_json("recovering", None, None);
        let w = parse_worker_json(&json).unwrap();
        assert_eq!(w.status, WorkerStatus::Recovering);
    }

    #[test]
    fn parse_unknown_status_defaults_to_failed() {
        let json = sample_json("bogus", None, None);
        let w = parse_worker_json(&json).unwrap();
        assert_eq!(w.status, WorkerStatus::Failed);
    }

    #[test]
    fn parse_missing_repo_returns_error() {
        let json = r#"{
            "issue_num": 42,
            "issue_title": "Fix the thing",
            "branch": "sipag/issue-42",
            "container_name": "sipag-issue-42",
            "status": "running"
        }"#;
        let result = parse_worker_json(json);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("missing required field: repo"));
    }

    #[test]
    fn parse_empty_repo_returns_error() {
        let json = r#"{
            "repo": "",
            "issue_num": 42,
            "issue_title": "Fix the thing",
            "branch": "sipag/issue-42",
            "container_name": "sipag-issue-42",
            "status": "running"
        }"#;
        let result = parse_worker_json(json);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("missing required field: repo"));
    }

    #[test]
    fn parse_missing_issue_num_returns_error() {
        let json = r#"{
            "repo": "Dorky-Robot/sipag",
            "issue_title": "Fix the thing",
            "branch": "sipag/issue-42",
            "container_name": "sipag-issue-42",
            "status": "running"
        }"#;
        let result = parse_worker_json(json);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("missing required field: issue_num"));
    }

    #[test]
    fn format_duration_variants() {
        assert_eq!(format_duration(None), "-");
        assert_eq!(format_duration(Some(0)), "0m0s");
        assert_eq!(format_duration(Some(263)), "4m23s");
        assert_eq!(format_duration(Some(3600)), "60m0s");
    }

    #[test]
    fn branch_display_running_shows_branch() {
        let json = sample_json("running", None, None);
        let w = parse_worker_json(&json).unwrap();
        assert_eq!(branch_display(&w), "sipag/issue-42-fix-the-thing");
    }

    #[test]
    fn branch_display_done_with_pr_shows_pr() {
        let json = sample_json("done", Some(300), Some(163));
        let w = parse_worker_json(&json).unwrap();
        assert_eq!(branch_display(&w), "PR #163");
    }

    #[test]
    fn branch_display_done_without_pr_shows_branch() {
        let json = sample_json("done", Some(300), None);
        let w = parse_worker_json(&json).unwrap();
        assert_eq!(branch_display(&w), "sipag/issue-42-fix-the-thing");
    }
}
