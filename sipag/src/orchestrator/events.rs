use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use sipag_core::state::{self, WorkerPhase};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

/// Events emitted by the file watcher for the orchestrator to react to.
#[derive(Debug)]
pub enum WorkEvent {
    WorkerFinished {
        repo: String,
        pr_num: u64,
    },
    WorkerFailed {
        repo: String,
        pr_num: u64,
    },
    WorkerStarted {
        repo: String,
        pr_num: u64,
    },
    WorkerStale {
        repo: String,
        pr_num: u64,
    },
    GithubPoll,
    #[allow(dead_code)]
    Shutdown,
}

/// Start the inline file watcher that monitors `~/.sipag/workers/` for state changes.
///
/// Returns a receiver for WorkEvent and the watcher handle (must be kept alive).
/// Spawns a background thread that watches for file changes, checks heartbeats,
/// and emits periodic GitHub poll events.
pub fn start_watcher(
    sipag_dir: &Path,
    poll_interval: u64,
    heartbeat_stale_secs: u64,
) -> anyhow::Result<(mpsc::Receiver<WorkEvent>, RecommendedWatcher)> {
    let workers_dir = sipag_dir.join("workers");
    std::fs::create_dir_all(&workers_dir)?;

    let (event_tx, event_rx) = mpsc::channel();
    let (notify_tx, notify_rx) = mpsc::channel();

    let mut watcher = RecommendedWatcher::new(notify_tx, notify::Config::default())?;
    watcher.watch(&workers_dir, RecursiveMode::NonRecursive)?;

    let tx = event_tx;
    let workers_dir_owned = workers_dir.clone();

    std::thread::spawn(move || {
        let mut tracker = PhaseTracker::new();
        tracker.seed(&workers_dir_owned);

        // Emit initial GitHub poll so the orchestrator starts its first cycle.
        let _ = tx.send(WorkEvent::GithubPoll);

        let poll_dur = Duration::from_secs(poll_interval);
        let heartbeat_dur = Duration::from_secs(heartbeat_stale_secs);
        let mut last_poll = Instant::now();
        let mut last_heartbeat = Instant::now();

        loop {
            match notify_rx.recv_timeout(Duration::from_secs(1)) {
                Ok(Ok(event)) => {
                    for work_event in tracker.handle_fs_event(&event, &workers_dir_owned) {
                        if tx.send(work_event).is_err() {
                            return; // receiver dropped
                        }
                    }
                }
                Ok(Err(_)) => {} // notify error, ignore
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }

            if last_poll.elapsed() >= poll_dur {
                if tx.send(WorkEvent::GithubPoll).is_err() {
                    return;
                }
                last_poll = Instant::now();
            }

            if last_heartbeat.elapsed() >= heartbeat_dur {
                for event in tracker.check_heartbeats(heartbeat_stale_secs) {
                    if tx.send(event).is_err() {
                        return;
                    }
                }
                last_heartbeat = Instant::now();
            }
        }
    });

    Ok((event_rx, watcher))
}

/// Tracks worker phases to detect transitions and emit appropriate events.
struct PhaseTracker {
    last_phases: HashMap<PathBuf, WorkerPhase>,
}

impl PhaseTracker {
    fn new() -> Self {
        Self {
            last_phases: HashMap::new(),
        }
    }

    /// Read all current state files so we don't emit events for pre-existing workers.
    fn seed(&mut self, workers_dir: &Path) {
        if let Ok(entries) = std::fs::read_dir(workers_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "json") {
                    if let Ok(s) = state::read_state(&path) {
                        self.last_phases.insert(path, s.phase);
                    }
                }
            }
        }
    }

    /// Handle a filesystem event, returning any work events to emit.
    fn handle_fs_event(&mut self, event: &notify::Event, workers_dir: &Path) -> Vec<WorkEvent> {
        let mut events = Vec::new();

        if matches!(event.kind, EventKind::Remove(_)) {
            for path in &event.paths {
                self.last_phases.remove(path);
            }
            return events;
        }

        match event.kind {
            EventKind::Create(_) | EventKind::Modify(_) => {}
            _ => return events,
        }

        for path in &event.paths {
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            if path.parent() != Some(workers_dir) {
                continue;
            }

            match state::read_state(path) {
                Ok(s) => {
                    let prev = self.last_phases.get(path);
                    if prev != Some(&s.phase) {
                        let work_event = match s.phase {
                            WorkerPhase::Starting | WorkerPhase::Working => {
                                WorkEvent::WorkerStarted {
                                    repo: s.repo.clone(),
                                    pr_num: s.pr_num,
                                }
                            }
                            WorkerPhase::Finished => WorkEvent::WorkerFinished {
                                repo: s.repo.clone(),
                                pr_num: s.pr_num,
                            },
                            WorkerPhase::Failed => WorkEvent::WorkerFailed {
                                repo: s.repo.clone(),
                                pr_num: s.pr_num,
                            },
                        };
                        events.push(work_event);
                        self.last_phases.insert(path.clone(), s.phase);
                    }
                }
                Err(_) => {
                    // Transient read failure (mid-write) — keep existing entry.
                }
            }
        }

        events
    }

    /// Check heartbeat files for non-terminal workers, emitting stale events.
    fn check_heartbeats(&mut self, stale_secs: u64) -> Vec<WorkEvent> {
        let stale_entries: Vec<_> = self
            .last_phases
            .iter()
            .filter(|(_, phase)| !phase.is_terminal())
            .filter_map(|(path, _)| {
                let heartbeat_path = path.with_extension("heartbeat");
                let metadata = std::fs::metadata(&heartbeat_path).ok()?;
                let modified = metadata.modified().ok()?;
                let age = SystemTime::now()
                    .duration_since(modified)
                    .unwrap_or_default();
                if age.as_secs() >= stale_secs {
                    if let Ok(s) = state::read_state(path) {
                        if !s.phase.is_terminal() {
                            return Some((path.clone(), s.repo, s.pr_num));
                        }
                    }
                }
                None
            })
            .collect();

        let mut events = Vec::new();
        for (path, repo, pr_num) in stale_entries {
            events.push(WorkEvent::WorkerStale { repo, pr_num });
            self.last_phases.insert(path, WorkerPhase::Failed);
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sipag_core::state::{WorkerPhase, WorkerState};
    use tempfile::TempDir;

    fn write_worker_state(dir: &Path, repo: &str, pr_num: u64, phase: WorkerPhase) -> PathBuf {
        let state = WorkerState {
            repo: repo.to_string(),
            pr_num,
            issues: vec![],
            branch: format!("sipag/pr-{pr_num}"),
            container_id: "test".to_string(),
            phase,
            heartbeat: "2026-01-01T00:00:00Z".to_string(),
            started: "2026-01-01T00:00:00Z".to_string(),
            ended: None,
            exit_code: None,
            error: None,
            file_path: state::state_file_path(dir, repo, pr_num),
        };
        state::write_state(&state).unwrap();
        state.file_path
    }

    #[test]
    fn phase_tracker_seed_loads_existing_workers() {
        let dir = TempDir::new().unwrap();
        let workers_dir = dir.path().join("workers");
        std::fs::create_dir_all(&workers_dir).unwrap();

        write_worker_state(dir.path(), "o/r", 1, WorkerPhase::Working);
        write_worker_state(dir.path(), "o/r", 2, WorkerPhase::Finished);

        let mut tracker = PhaseTracker::new();
        tracker.seed(&workers_dir);
        assert_eq!(tracker.last_phases.len(), 2);
    }

    #[test]
    fn phase_tracker_seed_empty_dir() {
        let dir = TempDir::new().unwrap();
        let workers_dir = dir.path().join("workers");
        std::fs::create_dir_all(&workers_dir).unwrap();

        let mut tracker = PhaseTracker::new();
        tracker.seed(&workers_dir);
        assert!(tracker.last_phases.is_empty());
    }

    #[test]
    fn phase_tracker_seed_ignores_non_json_files() {
        let dir = TempDir::new().unwrap();
        let workers_dir = dir.path().join("workers");
        std::fs::create_dir_all(&workers_dir).unwrap();
        std::fs::write(workers_dir.join("test.heartbeat"), "{}").unwrap();
        std::fs::write(workers_dir.join("test.txt"), "hello").unwrap();

        let mut tracker = PhaseTracker::new();
        tracker.seed(&workers_dir);
        assert!(tracker.last_phases.is_empty());
    }

    #[test]
    fn handle_fs_event_detects_finished() {
        let dir = TempDir::new().unwrap();
        let workers_dir = dir.path().join("workers");
        std::fs::create_dir_all(&workers_dir).unwrap();

        // Create a worker in "working" state.
        let path = write_worker_state(dir.path(), "owner/repo", 42, WorkerPhase::Working);

        let mut tracker = PhaseTracker::new();
        tracker.seed(&workers_dir);

        // Now transition to "finished".
        let mut state = state::read_state(&path).unwrap();
        state.phase = WorkerPhase::Finished;
        state::write_state(&state).unwrap();

        let event = notify::Event {
            kind: EventKind::Modify(notify::event::ModifyKind::Data(
                notify::event::DataChange::Content,
            )),
            paths: vec![path],
            attrs: Default::default(),
        };

        let events = tracker.handle_fs_event(&event, &workers_dir);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            WorkEvent::WorkerFinished { pr_num: 42, .. }
        ));
    }

    #[test]
    fn handle_fs_event_detects_failed() {
        let dir = TempDir::new().unwrap();
        let workers_dir = dir.path().join("workers");
        std::fs::create_dir_all(&workers_dir).unwrap();

        let path = write_worker_state(dir.path(), "owner/repo", 10, WorkerPhase::Working);
        let mut tracker = PhaseTracker::new();
        tracker.seed(&workers_dir);

        // Transition to failed.
        let mut state = state::read_state(&path).unwrap();
        state.phase = WorkerPhase::Failed;
        state::write_state(&state).unwrap();

        let event = notify::Event {
            kind: EventKind::Modify(notify::event::ModifyKind::Data(
                notify::event::DataChange::Content,
            )),
            paths: vec![path],
            attrs: Default::default(),
        };

        let events = tracker.handle_fs_event(&event, &workers_dir);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            WorkEvent::WorkerFailed { pr_num: 10, .. }
        ));
    }

    #[test]
    fn handle_fs_event_no_duplicate_for_same_phase() {
        let dir = TempDir::new().unwrap();
        let workers_dir = dir.path().join("workers");
        std::fs::create_dir_all(&workers_dir).unwrap();

        let path = write_worker_state(dir.path(), "owner/repo", 1, WorkerPhase::Working);
        let mut tracker = PhaseTracker::new();
        tracker.seed(&workers_dir);

        // Event for same phase — no change.
        let event = notify::Event {
            kind: EventKind::Modify(notify::event::ModifyKind::Data(
                notify::event::DataChange::Content,
            )),
            paths: vec![path],
            attrs: Default::default(),
        };

        let events = tracker.handle_fs_event(&event, &workers_dir);
        assert!(events.is_empty());
    }

    #[test]
    fn handle_fs_event_new_worker_emits_started() {
        let dir = TempDir::new().unwrap();
        let workers_dir = dir.path().join("workers");
        std::fs::create_dir_all(&workers_dir).unwrap();

        let mut tracker = PhaseTracker::new();
        tracker.seed(&workers_dir); // empty

        // Now create a new worker.
        let path = write_worker_state(dir.path(), "owner/repo", 5, WorkerPhase::Starting);

        let event = notify::Event {
            kind: EventKind::Create(notify::event::CreateKind::File),
            paths: vec![path],
            attrs: Default::default(),
        };

        let events = tracker.handle_fs_event(&event, &workers_dir);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            WorkEvent::WorkerStarted { pr_num: 5, .. }
        ));
    }

    #[test]
    fn handle_fs_event_remove_cleans_up_tracker() {
        let dir = TempDir::new().unwrap();
        let workers_dir = dir.path().join("workers");
        std::fs::create_dir_all(&workers_dir).unwrap();

        let path = write_worker_state(dir.path(), "owner/repo", 1, WorkerPhase::Finished);
        let mut tracker = PhaseTracker::new();
        tracker.seed(&workers_dir);
        assert_eq!(tracker.last_phases.len(), 1);

        let event = notify::Event {
            kind: EventKind::Remove(notify::event::RemoveKind::File),
            paths: vec![path],
            attrs: Default::default(),
        };

        tracker.handle_fs_event(&event, &workers_dir);
        assert!(tracker.last_phases.is_empty());
    }

    #[test]
    fn handle_fs_event_ignores_non_json() {
        let dir = TempDir::new().unwrap();
        let workers_dir = dir.path().join("workers");
        std::fs::create_dir_all(&workers_dir).unwrap();

        let mut tracker = PhaseTracker::new();
        let heartbeat_path = workers_dir.join("test.heartbeat");
        std::fs::write(&heartbeat_path, "{}").unwrap();

        let event = notify::Event {
            kind: EventKind::Create(notify::event::CreateKind::File),
            paths: vec![heartbeat_path],
            attrs: Default::default(),
        };

        let events = tracker.handle_fs_event(&event, &workers_dir);
        assert!(events.is_empty());
    }

    #[test]
    fn handle_fs_event_ignores_unrelated_events() {
        let dir = TempDir::new().unwrap();
        let workers_dir = dir.path().join("workers");
        std::fs::create_dir_all(&workers_dir).unwrap();

        let mut tracker = PhaseTracker::new();
        let event = notify::Event {
            kind: EventKind::Access(notify::event::AccessKind::Read),
            paths: vec![workers_dir.join("test.json")],
            attrs: Default::default(),
        };

        let events = tracker.handle_fs_event(&event, &workers_dir);
        assert!(events.is_empty());
    }

    #[test]
    fn check_heartbeats_no_stale_when_fresh() {
        let dir = TempDir::new().unwrap();
        let workers_dir = dir.path().join("workers");
        std::fs::create_dir_all(&workers_dir).unwrap();

        let path = write_worker_state(dir.path(), "owner/repo", 1, WorkerPhase::Working);

        // Write a fresh heartbeat.
        let heartbeat_path = path.with_extension("heartbeat");
        std::fs::write(&heartbeat_path, "{}").unwrap();

        let mut tracker = PhaseTracker::new();
        tracker.seed(&workers_dir);

        // With a very long stale threshold, nothing is stale.
        let events = tracker.check_heartbeats(9999);
        assert!(events.is_empty());
    }

    #[test]
    fn check_heartbeats_skips_terminal_workers() {
        let dir = TempDir::new().unwrap();
        let workers_dir = dir.path().join("workers");
        std::fs::create_dir_all(&workers_dir).unwrap();

        let path = write_worker_state(dir.path(), "owner/repo", 1, WorkerPhase::Finished);
        let heartbeat_path = path.with_extension("heartbeat");
        std::fs::write(&heartbeat_path, "{}").unwrap();

        let mut tracker = PhaseTracker::new();
        tracker.seed(&workers_dir);

        // Even with stale_secs=0 (everything stale), terminal workers are skipped.
        let events = tracker.check_heartbeats(0);
        assert!(events.is_empty());
    }

    #[test]
    fn start_watcher_emits_initial_github_poll() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();

        let (rx, _watcher) = start_watcher(dir.path(), 3600, 3600).unwrap();

        // The first event should be GithubPoll.
        let event = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(event, WorkEvent::GithubPoll));
    }

    // Note: Integration test for start_watcher detecting state changes is omitted
    // because filesystem notification latency varies across platforms (macOS kqueue
    // can be slow). The PhaseTracker unit tests above validate the event detection
    // logic thoroughly.
}
