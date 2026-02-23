use anyhow::Result;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use sipag_core::state::{self, WorkerPhase};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub fn run_watch(sipag_dir: &Path, github_interval: u64) -> Result<()> {
    let workers_dir = sipag_dir.join("workers");
    std::fs::create_dir_all(&workers_dir)?;

    let (tx, rx) = mpsc::channel();
    let mut watcher = RecommendedWatcher::new(tx, notify::Config::default())?;
    watcher.watch(&workers_dir, RecursiveMode::NonRecursive)?;

    let mut tracker = PhaseTracker::new();

    // Seed tracker with current state so we don't emit stale events on startup.
    tracker.seed(&workers_dir);

    // Emit initial GitHub poll so Claude starts its first cycle immediately.
    emit("SIPAG_GITHUB_POLL");

    let github_interval = Duration::from_secs(github_interval);
    let mut last_github_poll = Instant::now();

    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Ok(event)) => tracker.handle_event(&event, &workers_dir),
            Ok(Err(e)) => eprintln!("sipag watch: notify error: {e}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        if last_github_poll.elapsed() >= github_interval {
            emit("SIPAG_GITHUB_POLL");
            last_github_poll = Instant::now();
        }
    }

    Ok(())
}

fn emit(msg: &str) {
    println!("{msg}");
    std::io::stdout().flush().ok();
}

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

    fn handle_event(&mut self, event: &notify::Event, workers_dir: &Path) {
        // React to creates, modifies, and renames (atomic write = rename).
        match event.kind {
            EventKind::Create(_) | EventKind::Modify(_) => {}
            _ => return,
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
                        let tag = match s.phase {
                            WorkerPhase::Starting => "SIPAG_WORKER_STARTED",
                            WorkerPhase::Working => "SIPAG_WORKER_WORKING",
                            WorkerPhase::Finished => "SIPAG_WORKER_FINISHED",
                            WorkerPhase::Failed => "SIPAG_WORKER_FAILED",
                        };
                        emit(&format!("{tag} {} {}", s.repo, s.pr_num));
                        self.last_phases.insert(path.clone(), s.phase);
                    }
                }
                Err(_) => {
                    // File removed or mid-write — clean up tracker.
                    self.last_phases.remove(path);
                }
            }
        }
    }
}
