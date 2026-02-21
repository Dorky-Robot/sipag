use anyhow::Result;
use std::path::{Path, PathBuf};
use std::{fs, io};

use super::ports::StateStore;
use super::state::{parse_worker_json, WorkerState};

/// Filesystem-backed state store reading from `<sipag_dir>/workers/*.json`.
pub struct FileStateStore {
    workers_dir: PathBuf,
}

impl FileStateStore {
    pub fn new(sipag_dir: &Path) -> Self {
        Self {
            workers_dir: sipag_dir.join("workers"),
        }
    }

    fn state_file_path(&self, repo_slug: &str, issue_num: u64) -> PathBuf {
        self.workers_dir
            .join(format!("{}--{}.json", repo_slug, issue_num))
    }
}

impl StateStore for FileStateStore {
    fn load(&self, repo_slug: &str, issue_num: u64) -> Result<Option<WorkerState>> {
        let path = self.state_file_path(repo_slug, issue_num);
        match fs::read_to_string(&path) {
            Ok(content) => Ok(Some(parse_worker_json(&content)?)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn save(&self, state: &WorkerState) -> Result<()> {
        fs::create_dir_all(&self.workers_dir)?;
        let repo_slug = state.repo.replace('/', "--");
        let path = self.state_file_path(&repo_slug, state.issue_num);

        let json = serde_json::json!({
            "repo": state.repo,
            "issue_num": state.issue_num,
            "issue_title": state.issue_title,
            "branch": state.branch,
            "container_name": state.container_name,
            "pr_num": state.pr_num,
            "pr_url": state.pr_url,
            "status": state.status.as_str(),
            "started_at": state.started_at,
            "ended_at": state.ended_at,
            "duration_s": state.duration_s,
            "exit_code": state.exit_code,
            "log_path": state.log_path.as_ref().map(|p| p.display().to_string()),
            "last_heartbeat": state.last_heartbeat,
            "phase": state.phase,
        });

        let content = serde_json::to_string_pretty(&json)?;
        fs::write(&path, content)?;
        Ok(())
    }

    fn list_active(&self) -> Result<Vec<WorkerState>> {
        if !self.workers_dir.exists() {
            return Ok(vec![]);
        }

        let mut active = vec![];
        for entry in fs::read_dir(&self.workers_dir)?.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(state) = parse_worker_json(&content) {
                        if state.status.is_active() {
                            active.push(state);
                        }
                    }
                }
            }
        }
        active.sort_by_key(|w| w.issue_num);
        Ok(active)
    }
}

/// Read all worker state files (any status) for the TUI and `sipag ps`.
///
/// Backward-compatible standalone function. Equivalent to the old
/// `list_workers()` but delegates to the new parsing logic.
pub fn list_all_workers(sipag_dir: &Path) -> Result<Vec<WorkerState>> {
    let workers_dir = sipag_dir.join("workers");
    if !workers_dir.exists() {
        return Ok(vec![]);
    }

    let mut paths: Vec<PathBuf> = fs::read_dir(&workers_dir)?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .map(|e| e.path())
        .collect();
    paths.sort();

    let mut workers = vec![];
    for path in paths {
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(state) = parse_worker_json(&content) {
                workers.push(state);
            }
        }
    }

    Ok(workers)
}

/// Update the status of a worker to "failed" by container name.
///
/// Backward-compatible standalone function used by the TUI's kill action.
pub fn mark_worker_failed_by_container(sipag_dir: &Path, container_name: &str) -> Result<()> {
    let workers_dir = sipag_dir.join("workers");
    if !workers_dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(&workers_dir)?.flatten() {
        let path = entry.path();
        if path.extension().map(|x| x != "json").unwrap_or(true) {
            continue;
        }
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let mut v: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v["container_name"].as_str() == Some(container_name) {
            v["status"] = serde_json::Value::String("failed".to_string());
            if let Ok(updated) = serde_json::to_string_pretty(&v) {
                let _ = fs::write(&path, updated);
            }
            return Ok(());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::status::WorkerStatus;
    use super::*;
    use std::io::Write as IoWrite;
    use tempfile::TempDir;

    fn sample_json(issue_num: u64, status: &str) -> String {
        format!(
            r#"{{
                "repo": "test/repo",
                "issue_num": {issue_num},
                "issue_title": "Issue {issue_num}",
                "branch": "sipag/issue-{issue_num}-test",
                "container_name": "sipag-issue-{issue_num}",
                "pr_num": null,
                "pr_url": null,
                "status": "{status}",
                "started_at": "2024-01-01T00:00:00Z",
                "ended_at": null,
                "duration_s": null,
                "exit_code": null,
                "log_path": null
            }}"#
        )
    }

    #[test]
    fn load_existing_worker() {
        let dir = TempDir::new().unwrap();
        let workers_dir = dir.path().join("workers");
        fs::create_dir(&workers_dir).unwrap();
        fs::write(
            workers_dir.join("test--repo--42.json"),
            sample_json(42, "running"),
        )
        .unwrap();

        let store = FileStateStore::new(dir.path());
        let worker = store.load("test--repo", 42).unwrap().unwrap();
        assert_eq!(worker.issue_num, 42);
        assert_eq!(worker.status, WorkerStatus::Running);
    }

    #[test]
    fn load_missing_worker_returns_none() {
        let dir = TempDir::new().unwrap();
        let store = FileStateStore::new(dir.path());
        assert!(store.load("test--repo", 999).unwrap().is_none());
    }

    #[test]
    fn save_creates_file() {
        let dir = TempDir::new().unwrap();
        let store = FileStateStore::new(dir.path());

        let state = WorkerState {
            repo: "test/repo".to_string(),
            issue_num: 42,
            issue_title: "Test issue".to_string(),
            branch: "sipag/issue-42-test".to_string(),
            container_name: "sipag-issue-42".to_string(),
            pr_num: Some(100),
            pr_url: Some("https://github.com/test/repo/pull/100".to_string()),
            status: WorkerStatus::Done,
            started_at: Some("2024-01-01T00:00:00Z".to_string()),
            ended_at: Some("2024-01-01T01:00:00Z".to_string()),
            duration_s: Some(3600),
            exit_code: Some(0),
            log_path: None,
            last_heartbeat: None,
            phase: None,
        };

        store.save(&state).unwrap();

        let loaded = store.load("test--repo", 42).unwrap().unwrap();
        assert_eq!(loaded.status, WorkerStatus::Done);
        assert_eq!(loaded.pr_num, Some(100));
        assert_eq!(loaded.issue_title, "Test issue");
    }

    #[test]
    fn save_overwrites_existing() {
        let dir = TempDir::new().unwrap();
        let workers_dir = dir.path().join("workers");
        fs::create_dir(&workers_dir).unwrap();
        fs::write(
            workers_dir.join("test--repo--42.json"),
            sample_json(42, "running"),
        )
        .unwrap();

        let store = FileStateStore::new(dir.path());
        let mut state = store.load("test--repo", 42).unwrap().unwrap();
        state.status = WorkerStatus::Done;
        state.pr_num = Some(100);
        store.save(&state).unwrap();

        let reloaded = store.load("test--repo", 42).unwrap().unwrap();
        assert_eq!(reloaded.status, WorkerStatus::Done);
        assert_eq!(reloaded.pr_num, Some(100));
    }

    #[test]
    fn list_active_filters_terminal() {
        let dir = TempDir::new().unwrap();
        let workers_dir = dir.path().join("workers");
        fs::create_dir(&workers_dir).unwrap();

        fs::write(
            workers_dir.join("test--repo--1.json"),
            sample_json(1, "running"),
        )
        .unwrap();
        fs::write(
            workers_dir.join("test--repo--2.json"),
            sample_json(2, "done"),
        )
        .unwrap();
        fs::write(
            workers_dir.join("test--repo--3.json"),
            sample_json(3, "failed"),
        )
        .unwrap();
        fs::write(
            workers_dir.join("test--repo--4.json"),
            sample_json(4, "recovering"),
        )
        .unwrap();

        let store = FileStateStore::new(dir.path());
        let active = store.list_active().unwrap();

        assert_eq!(active.len(), 2);
        assert_eq!(active[0].issue_num, 1); // running
        assert_eq!(active[1].issue_num, 4); // recovering
    }

    #[test]
    fn list_active_empty_dir() {
        let dir = TempDir::new().unwrap();
        let store = FileStateStore::new(dir.path());
        assert!(store.list_active().unwrap().is_empty());
    }

    #[test]
    fn list_all_workers_reads_all_statuses() {
        let dir = TempDir::new().unwrap();
        let workers_dir = dir.path().join("workers");
        fs::create_dir(&workers_dir).unwrap();

        let mut f = fs::File::create(workers_dir.join("test--repo--1.json")).unwrap();
        writeln!(f, "{}", sample_json(1, "running")).unwrap();
        let mut f = fs::File::create(workers_dir.join("test--repo--2.json")).unwrap();
        writeln!(f, "{}", sample_json(2, "done")).unwrap();

        let workers = list_all_workers(dir.path()).unwrap();
        assert_eq!(workers.len(), 2);
    }

    #[test]
    fn list_all_workers_missing_dir() {
        let dir = TempDir::new().unwrap();
        let workers = list_all_workers(dir.path()).unwrap();
        assert!(workers.is_empty());
    }

    #[test]
    fn list_all_workers_skips_invalid_json() {
        let dir = TempDir::new().unwrap();
        let workers_dir = dir.path().join("workers");
        fs::create_dir(&workers_dir).unwrap();

        fs::write(workers_dir.join("bad.json"), "not json").unwrap();
        let mut f = fs::File::create(workers_dir.join("good.json")).unwrap();
        writeln!(f, "{}", sample_json(1, "done")).unwrap();

        let workers = list_all_workers(dir.path()).unwrap();
        assert_eq!(workers.len(), 1);
    }

    #[test]
    fn mark_worker_failed_by_container_updates_status() {
        let dir = TempDir::new().unwrap();
        let workers_dir = dir.path().join("workers");
        fs::create_dir(&workers_dir).unwrap();
        fs::write(
            workers_dir.join("test--repo--42.json"),
            sample_json(42, "running"),
        )
        .unwrap();

        mark_worker_failed_by_container(dir.path(), "sipag-issue-42").unwrap();

        let content = fs::read_to_string(workers_dir.join("test--repo--42.json")).unwrap();
        let state = parse_worker_json(&content).unwrap();
        assert_eq!(state.status, WorkerStatus::Failed);
    }

    #[test]
    fn mark_worker_failed_by_container_noop_for_unknown() {
        let dir = TempDir::new().unwrap();
        mark_worker_failed_by_container(dir.path(), "nonexistent").unwrap();
    }
}
