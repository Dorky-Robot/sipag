use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Orchestrator phase — only tracks lifecycle, not individual event handlers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkPhase {
    Startup,
    Running,
    Done,
}

/// Outcome of a PR review — returned by the review handler.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum ReviewOutcome {
    Merged,
    NeedsRedispatch,
    Escalate,
    Skipped,
}

/// A disease cluster identified during analysis — groups related issues by root cause.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiseaseCluster {
    pub name: String,
    pub description: String,
    pub issues: Vec<u64>,
    pub affected_files: Vec<String>,
    pub fix_approach: String,
}

/// Snapshot of a resolved repo captured at session start.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoSnapshot {
    pub full_name: String,
    pub local_path: String,
}

/// Persistent session state — saved only on phase transitions (Startup → Running → Done).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub phase: WorkPhase,
    pub repos: Vec<RepoSnapshot>,
    pub started: String,
    pub last_transition: String,
}

impl SessionState {
    pub fn new(repos: &[sipag_core::repo::ResolvedRepo]) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            phase: WorkPhase::Startup,
            repos: repos
                .iter()
                .map(|r| RepoSnapshot {
                    full_name: r.full_name.clone(),
                    local_path: r.local_path.display().to_string(),
                })
                .collect(),
            started: now.clone(),
            last_transition: now,
        }
    }

    /// Persist session state to `{sipag_dir}/session.json`.
    pub fn save(&self, sipag_dir: &Path) -> Result<()> {
        let path = sipag_dir.join("session.json");
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    /// Load session state from a JSON file.
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&content)?)
    }

    /// Transition to a new phase, updating the timestamp.
    pub fn transition(&mut self, next: WorkPhase) {
        self.phase = next;
        self.last_transition = chrono::Utc::now().to_rfc3339();
    }
}

/// Check if a resumable session exists and return its path.
pub fn find_resumable_session(sipag_dir: &Path) -> Option<PathBuf> {
    let path = sipag_dir.join("session.json");
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

/// Save disease clusters to `{sipag_dir}/diseases/{owner}--{repo}.json`.
pub fn save_diseases(sipag_dir: &Path, repo: &str, clusters: &[DiseaseCluster]) -> Result<()> {
    let diseases_dir = sipag_dir.join("diseases");
    std::fs::create_dir_all(&diseases_dir)?;
    let slug = repo.replace('/', "--");
    let path = diseases_dir.join(format!("{slug}.json"));
    let json = serde_json::to_string_pretty(clusters)?;
    std::fs::write(&path, json)?;
    Ok(())
}

/// Load disease clusters from `{sipag_dir}/diseases/{owner}--{repo}.json`.
///
/// Returns empty vec if the file doesn't exist — poll works fine without diseases.
pub fn load_diseases(sipag_dir: &Path, repo: &str) -> Vec<DiseaseCluster> {
    let slug = repo.replace('/', "--");
    let path = sipag_dir.join("diseases").join(format!("{slug}.json"));
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn session_state_save_and_load() {
        let dir = TempDir::new().unwrap();
        let session = SessionState {
            phase: WorkPhase::Startup,
            repos: vec![RepoSnapshot {
                full_name: "owner/repo".to_string(),
                local_path: "/tmp/repo".to_string(),
            }],
            started: "2026-01-01T00:00:00Z".to_string(),
            last_transition: "2026-01-01T00:00:00Z".to_string(),
        };

        session.save(dir.path()).unwrap();

        let loaded = SessionState::load(&dir.path().join("session.json")).unwrap();
        assert_eq!(loaded.repos.len(), 1);
        assert_eq!(loaded.repos[0].full_name, "owner/repo");
        assert!(matches!(loaded.phase, WorkPhase::Startup));
    }

    #[test]
    fn session_state_transition_updates_timestamp() {
        let mut session = SessionState {
            phase: WorkPhase::Startup,
            repos: Vec::new(),
            started: "2026-01-01T00:00:00Z".to_string(),
            last_transition: "2026-01-01T00:00:00Z".to_string(),
        };

        let before = session.last_transition.clone();
        session.transition(WorkPhase::Running);
        assert!(matches!(session.phase, WorkPhase::Running));
        assert_ne!(session.last_transition, before);
    }

    #[test]
    fn find_resumable_session_returns_none_when_missing() {
        let dir = TempDir::new().unwrap();
        assert!(find_resumable_session(dir.path()).is_none());
    }

    #[test]
    fn find_resumable_session_returns_path_when_exists() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("session.json"), "{}").unwrap();
        assert!(find_resumable_session(dir.path()).is_some());
    }

    #[test]
    fn work_phase_serialization_round_trip() {
        let phases = vec![WorkPhase::Startup, WorkPhase::Running, WorkPhase::Done];

        for phase in phases {
            let json = serde_json::to_string(&phase).unwrap();
            let loaded: WorkPhase = serde_json::from_str(&json).unwrap();
            assert_eq!(format!("{:?}", phase), format!("{:?}", loaded));
        }
    }

    #[test]
    fn disease_cluster_serialization() {
        let cluster = DiseaseCluster {
            name: "Missing error handling".to_string(),
            description: "No unified error type".to_string(),
            issues: vec![1, 2, 3],
            affected_files: vec!["src/lib.rs".to_string()],
            fix_approach: "Add anyhow error type".to_string(),
        };

        let json = serde_json::to_string(&cluster).unwrap();
        let loaded: DiseaseCluster = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.name, "Missing error handling");
        assert_eq!(loaded.issues, vec![1, 2, 3]);
    }

    #[test]
    fn session_state_save_creates_file() {
        let dir = TempDir::new().unwrap();
        let session = SessionState {
            phase: WorkPhase::Startup,
            repos: Vec::new(),
            started: "2026-01-01T00:00:00Z".to_string(),
            last_transition: "2026-01-01T00:00:00Z".to_string(),
        };

        session.save(dir.path()).unwrap();
        assert!(dir.path().join("session.json").exists());
    }

    #[test]
    fn session_state_save_is_valid_json() {
        let dir = TempDir::new().unwrap();
        let session = SessionState {
            phase: WorkPhase::Running,
            repos: vec![RepoSnapshot {
                full_name: "owner/repo".to_string(),
                local_path: "/path/to/repo".to_string(),
            }],
            started: "2026-01-01T00:00:00Z".to_string(),
            last_transition: "2026-01-15T12:00:00Z".to_string(),
        };

        session.save(dir.path()).unwrap();

        let content = std::fs::read_to_string(dir.path().join("session.json")).unwrap();
        let _: serde_json::Value = serde_json::from_str(&content).unwrap();
    }

    #[test]
    fn session_state_multiple_repos_round_trip() {
        let dir = TempDir::new().unwrap();
        let session = SessionState {
            phase: WorkPhase::Running,
            repos: vec![
                RepoSnapshot {
                    full_name: "owner/repo-a".to_string(),
                    local_path: "/a".to_string(),
                },
                RepoSnapshot {
                    full_name: "owner/repo-b".to_string(),
                    local_path: "/b".to_string(),
                },
            ],
            started: "2026-01-01T00:00:00Z".to_string(),
            last_transition: "2026-01-01T00:00:00Z".to_string(),
        };

        session.save(dir.path()).unwrap();
        let loaded = SessionState::load(&dir.path().join("session.json")).unwrap();
        assert_eq!(loaded.repos.len(), 2);
        assert_eq!(loaded.repos[0].full_name, "owner/repo-a");
        assert_eq!(loaded.repos[1].full_name, "owner/repo-b");
    }

    #[test]
    fn session_state_save_overwrites_previous() {
        let dir = TempDir::new().unwrap();

        let mut session = SessionState {
            phase: WorkPhase::Startup,
            repos: Vec::new(),
            started: "2026-01-01T00:00:00Z".to_string(),
            last_transition: "2026-01-01T00:00:00Z".to_string(),
        };
        session.save(dir.path()).unwrap();

        session.transition(WorkPhase::Running);
        session.save(dir.path()).unwrap();

        let loaded = SessionState::load(&dir.path().join("session.json")).unwrap();
        assert!(matches!(loaded.phase, WorkPhase::Running));
    }

    #[test]
    fn transition_preserves_other_fields() {
        let mut session = SessionState {
            phase: WorkPhase::Startup,
            repos: vec![RepoSnapshot {
                full_name: "o/r".to_string(),
                local_path: "/p".to_string(),
            }],
            started: "2026-01-01T00:00:00Z".to_string(),
            last_transition: "2026-01-01T00:00:00Z".to_string(),
        };

        session.transition(WorkPhase::Running);

        assert_eq!(session.repos.len(), 1);
        assert_eq!(session.started, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn session_load_malformed_json_returns_error() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("session.json"), "not json{{{").unwrap();
        assert!(SessionState::load(&dir.path().join("session.json")).is_err());
    }

    #[test]
    fn session_load_missing_file_returns_error() {
        let dir = TempDir::new().unwrap();
        assert!(SessionState::load(&dir.path().join("nonexistent.json")).is_err());
    }

    #[test]
    fn save_and_load_diseases() {
        let dir = TempDir::new().unwrap();
        let clusters = vec![DiseaseCluster {
            name: "test disease".to_string(),
            description: "desc".to_string(),
            issues: vec![1, 2],
            affected_files: vec!["a.rs".to_string()],
            fix_approach: "fix it".to_string(),
        }];

        save_diseases(dir.path(), "owner/repo", &clusters).unwrap();
        let loaded = load_diseases(dir.path(), "owner/repo");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "test disease");
        assert_eq!(loaded[0].issues, vec![1, 2]);
    }

    #[test]
    fn load_diseases_returns_empty_when_missing() {
        let dir = TempDir::new().unwrap();
        let loaded = load_diseases(dir.path(), "owner/repo");
        assert!(loaded.is_empty());
    }

    #[test]
    fn save_diseases_creates_dir() {
        let dir = TempDir::new().unwrap();
        save_diseases(dir.path(), "owner/repo", &[]).unwrap();
        assert!(dir.path().join("diseases").exists());
    }
}
