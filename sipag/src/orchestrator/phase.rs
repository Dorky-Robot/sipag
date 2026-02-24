use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Orchestrator phase — the current step in the sipag work cycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkPhase {
    Init,
    PruneStaleIssues {
        repo_index: usize,
    },
    AnalyzeDiseases {
        repo_index: usize,
    },
    RecoverInFlight {
        repo_index: usize,
    },
    EventLoop,
    ReviewPr {
        repo: String,
        pr_num: u64,
        attempt: u8,
    },
    HandleFailed {
        repo: String,
        pr_num: u64,
    },
    HandleStale {
        repo: String,
        pr_num: u64,
    },
    PollCycle,
    Retro,
    Done,
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

/// Persistent session state — saved after every phase transition for crash recovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub phase: WorkPhase,
    pub repos: Vec<RepoSnapshot>,
    pub diseases: Vec<DiseaseCluster>,
    pub workers_completed_since_retro: u32,
    pub started: String,
    pub last_transition: String,
}

impl SessionState {
    pub fn new(repos: &[sipag_core::repo::ResolvedRepo]) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            phase: WorkPhase::Init,
            repos: repos
                .iter()
                .map(|r| RepoSnapshot {
                    full_name: r.full_name.clone(),
                    local_path: r.local_path.display().to_string(),
                })
                .collect(),
            diseases: Vec::new(),
            workers_completed_since_retro: 0,
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn session_state_save_and_load() {
        let dir = TempDir::new().unwrap();
        let session = SessionState {
            phase: WorkPhase::Init,
            repos: vec![RepoSnapshot {
                full_name: "owner/repo".to_string(),
                local_path: "/tmp/repo".to_string(),
            }],
            diseases: Vec::new(),
            workers_completed_since_retro: 0,
            started: "2026-01-01T00:00:00Z".to_string(),
            last_transition: "2026-01-01T00:00:00Z".to_string(),
        };

        session.save(dir.path()).unwrap();

        let loaded = SessionState::load(&dir.path().join("session.json")).unwrap();
        assert_eq!(loaded.repos.len(), 1);
        assert_eq!(loaded.repos[0].full_name, "owner/repo");
        assert!(matches!(loaded.phase, WorkPhase::Init));
    }

    #[test]
    fn session_state_transition_updates_timestamp() {
        let mut session = SessionState {
            phase: WorkPhase::Init,
            repos: Vec::new(),
            diseases: Vec::new(),
            workers_completed_since_retro: 0,
            started: "2026-01-01T00:00:00Z".to_string(),
            last_transition: "2026-01-01T00:00:00Z".to_string(),
        };

        let before = session.last_transition.clone();
        session.transition(WorkPhase::EventLoop);
        assert!(matches!(session.phase, WorkPhase::EventLoop));
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
        let phases = vec![
            WorkPhase::Init,
            WorkPhase::PruneStaleIssues { repo_index: 2 },
            WorkPhase::ReviewPr {
                repo: "o/r".to_string(),
                pr_num: 42,
                attempt: 1,
            },
            WorkPhase::EventLoop,
            WorkPhase::Done,
        ];

        for phase in phases {
            let json = serde_json::to_string(&phase).unwrap();
            let loaded: WorkPhase = serde_json::from_str(&json).unwrap();
            // Just verify it round-trips without panic.
            let _ = format!("{:?}", loaded);
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
    fn all_work_phase_variants_serialize() {
        let phases = vec![
            WorkPhase::Init,
            WorkPhase::PruneStaleIssues { repo_index: 0 },
            WorkPhase::PruneStaleIssues { repo_index: 99 },
            WorkPhase::AnalyzeDiseases { repo_index: 0 },
            WorkPhase::RecoverInFlight { repo_index: 0 },
            WorkPhase::EventLoop,
            WorkPhase::ReviewPr {
                repo: "owner/repo".to_string(),
                pr_num: 1,
                attempt: 0,
            },
            WorkPhase::HandleFailed {
                repo: "o/r".to_string(),
                pr_num: 2,
            },
            WorkPhase::HandleStale {
                repo: "o/r".to_string(),
                pr_num: 3,
            },
            WorkPhase::PollCycle,
            WorkPhase::Retro,
            WorkPhase::Done,
        ];

        for phase in &phases {
            let json = serde_json::to_string(phase).unwrap();
            let loaded: WorkPhase = serde_json::from_str(&json).unwrap();
            // Verify debug output matches (proxy for equality since we don't have PartialEq).
            assert_eq!(format!("{:?}", phase), format!("{:?}", loaded));
        }
    }

    #[test]
    fn session_state_save_creates_file() {
        let dir = TempDir::new().unwrap();
        let session = SessionState {
            phase: WorkPhase::Init,
            repos: Vec::new(),
            diseases: Vec::new(),
            workers_completed_since_retro: 0,
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
            phase: WorkPhase::ReviewPr {
                repo: "owner/repo".to_string(),
                pr_num: 42,
                attempt: 1,
            },
            repos: vec![RepoSnapshot {
                full_name: "owner/repo".to_string(),
                local_path: "/path/to/repo".to_string(),
            }],
            diseases: vec![DiseaseCluster {
                name: "test".to_string(),
                description: "desc".to_string(),
                issues: vec![1],
                affected_files: vec!["a.rs".to_string()],
                fix_approach: "fix".to_string(),
            }],
            workers_completed_since_retro: 5,
            started: "2026-01-01T00:00:00Z".to_string(),
            last_transition: "2026-01-15T12:00:00Z".to_string(),
        };

        session.save(dir.path()).unwrap();

        // Verify it's valid JSON.
        let content = std::fs::read_to_string(dir.path().join("session.json")).unwrap();
        let _: serde_json::Value = serde_json::from_str(&content).unwrap();
    }

    #[test]
    fn session_state_multiple_repos_round_trip() {
        let dir = TempDir::new().unwrap();
        let session = SessionState {
            phase: WorkPhase::PruneStaleIssues { repo_index: 1 },
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
            diseases: Vec::new(),
            workers_completed_since_retro: 0,
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
            phase: WorkPhase::Init,
            repos: Vec::new(),
            diseases: Vec::new(),
            workers_completed_since_retro: 0,
            started: "2026-01-01T00:00:00Z".to_string(),
            last_transition: "2026-01-01T00:00:00Z".to_string(),
        };
        session.save(dir.path()).unwrap();

        session.transition(WorkPhase::EventLoop);
        session.workers_completed_since_retro = 3;
        session.save(dir.path()).unwrap();

        let loaded = SessionState::load(&dir.path().join("session.json")).unwrap();
        assert!(matches!(loaded.phase, WorkPhase::EventLoop));
        assert_eq!(loaded.workers_completed_since_retro, 3);
    }

    #[test]
    fn transition_preserves_other_fields() {
        let mut session = SessionState {
            phase: WorkPhase::Init,
            repos: vec![RepoSnapshot {
                full_name: "o/r".to_string(),
                local_path: "/p".to_string(),
            }],
            diseases: vec![DiseaseCluster {
                name: "d".to_string(),
                description: "d".to_string(),
                issues: vec![1],
                affected_files: Vec::new(),
                fix_approach: "f".to_string(),
            }],
            workers_completed_since_retro: 7,
            started: "2026-01-01T00:00:00Z".to_string(),
            last_transition: "2026-01-01T00:00:00Z".to_string(),
        };

        session.transition(WorkPhase::PollCycle);

        assert_eq!(session.repos.len(), 1);
        assert_eq!(session.diseases.len(), 1);
        assert_eq!(session.workers_completed_since_retro, 7);
        assert_eq!(session.started, "2026-01-01T00:00:00Z");
    }
}
