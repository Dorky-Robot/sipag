use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::cycle::{plan_cycle, IssueSnapshot};
use super::dispatcher::WorkerDispatcher;
use super::docker_runtime::DockerCliRuntime;
use super::drain::DrainSignal;
use super::gh_gateway::{GhCliGateway, WorkerPoller};
use super::ports::{GitHubGateway, StateStore};
use super::recovery::recover_and_finalize;
use super::state::WorkerState;
use super::status::WorkerStatus;
use super::store::FileStateStore;
use super::work_config::WorkerConfig;

/// Lifecycle state of the worker polling loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopState {
    /// Actively polling for new work.
    Polling,
    /// Dispatching a batch of work items.
    Dispatching,
    /// Drain signal received — finishing in-flight work but not picking up new issues.
    Draining,
    /// Loop has exited cleanly.
    Stopped,
}

/// The worker polling loop — typed state machine.
///
/// Replaces `lib/worker/loop.sh`. The loop:
///   1. Recovers orphaned containers from previous crashes.
///   2. Each cycle: checks drain signal, finalizes exited containers,
///      reconciles merged PRs, auto-merges clean PRs, calls
///      `plan_cycle()` to determine work, and dispatches.
///   3. Sleeps `poll_interval` between cycles.
///   4. Exits on drain signal or `--once` flag.
pub struct WorkerLoop {
    /// Repositories to poll (in "owner/repo" format).
    repos: Vec<String>,
    /// Resolved configuration.
    config: WorkerConfig,
    /// Current lifecycle state.
    state: LoopState,
    /// Cycle counter (starts at 0).
    cycle_count: u64,
}

impl WorkerLoop {
    /// Create a new WorkerLoop.
    pub fn new(repos: Vec<String>, config: WorkerConfig) -> Self {
        Self {
            repos,
            config,
            state: LoopState::Polling,
            cycle_count: 0,
        }
    }

    /// Run the polling loop until drain signal or `--once`.
    pub fn run(&mut self) -> Result<()> {
        let sipag_dir = self.config.sipag_dir.clone();
        let drain = DrainSignal::new(&sipag_dir);
        let store = FileStateStore::new(&sipag_dir);
        let containers = DockerCliRuntime;
        let github = GhCliGateway;

        // Create workers dir if needed
        std::fs::create_dir_all(sipag_dir.join("workers"))?;
        std::fs::create_dir_all(sipag_dir.join("logs"))?;

        // Print startup banner
        println!("sipag work");
        if self.repos.len() == 1 {
            println!("Repo: {}", self.repos[0]);
        } else {
            println!("Repos ({}): {}", self.repos.len(), self.repos.join(", "));
        }
        println!("Label: {}", self.config.work_label);
        println!("Batch size: {}", self.config.batch_size);
        println!("Poll interval: {}s", self.config.poll_interval.as_secs());
        println!("Logs: {}/logs/", sipag_dir.display());
        println!(
            "Started: {}",
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
        );
        println!();

        // Recover orphaned containers from previous crash
        let recovery_outcomes =
            recover_and_finalize(&containers, &github, &store, &self.config.work_label)?;
        if !recovery_outcomes.is_empty() {
            println!(
                "[recovery] Processed {} orphaned worker(s)",
                recovery_outcomes.len()
            );
        }

        // In-memory tracking for PR iteration and conflict-fix state.
        // Reset on restart (same behavior as bash temp files).
        let mut running_iteration_prs: HashSet<u64> = HashSet::new();
        let mut running_conflict_fix_prs: HashSet<u64> = HashSet::new();

        loop {
            // Check drain signal
            if drain.is_set() {
                println!(
                    "[{}] Drain signal detected. Finishing in-flight work, not picking up new issues.",
                    timestamp()
                );
                self.state = LoopState::Draining;
                break;
            }

            self.state = LoopState::Polling;

            // Finalize exited containers
            let _ = recover_and_finalize(&containers, &github, &store, &self.config.work_label);

            let mut found_work = false;

            for repo in &self.repos.clone() {
                // Reconcile: close issues whose PRs have been merged
                if let Err(e) = github.reconcile_merged_prs(repo, &self.config.work_label) {
                    eprintln!("[{}] reconcile error for {repo}: {e}", timestamp());
                }

                // Auto-merge clean sipag PRs
                if let Err(e) = github.auto_merge_clean_prs(repo) {
                    eprintln!("[{}] auto-merge error for {repo}: {e}", timestamp());
                }

                // Collect state for planning
                let active_workers = store.list_active().unwrap_or_default();
                let worker_statuses: HashMap<u64, WorkerStatus> =
                    build_status_map(&active_workers, repo);

                // List approved issues
                let approved_issues = github
                    .list_approved_issues(repo, &self.config.work_label)
                    .unwrap_or_default();

                // Build issue snapshots (check for existing PRs)
                let issue_snapshots: Vec<IssueSnapshot> = approved_issues
                    .iter()
                    .map(|&num| {
                        let has_existing_pr = if worker_statuses.contains_key(&num) {
                            // If we have a state file, trust it; avoid extra gh calls
                            false
                        } else {
                            github.has_pr_for_issue(repo, num).unwrap_or(false)
                        };
                        IssueSnapshot {
                            issue_num: num,
                            has_existing_pr,
                        }
                    })
                    .collect();

                // Find PRs needing iteration and conflicted PRs
                let prs_needing_iteration =
                    github.find_prs_needing_iteration(repo).unwrap_or_default();
                let conflicted_prs = github.find_conflicted_prs(repo).unwrap_or_default();

                let running_iter_vec: Vec<u64> = running_iteration_prs.iter().copied().collect();
                let running_fix_vec: Vec<u64> = running_conflict_fix_prs.iter().copied().collect();

                // Plan the cycle (pure function)
                let plan = plan_cycle(
                    &issue_snapshots,
                    &worker_statuses,
                    &prs_needing_iteration,
                    &conflicted_prs,
                    &running_iter_vec,
                    &running_fix_vec,
                    self.config.batch_size,
                    false, // drain already checked above
                );

                if plan.has_dispatch() {
                    found_work = true;
                }

                // Record issues as done (existing PR, no state file)
                for &issue_num in &plan.record_as_done {
                    let pr = github
                        .find_pr_for_branch(repo, &format!("sipag/issue-{issue_num}-"))
                        .ok()
                        .flatten();
                    let done_state = WorkerState {
                        repo: repo.clone(),
                        issue_num,
                        issue_title: String::new(),
                        branch: String::new(),
                        container_name: String::new(),
                        pr_num: pr.as_ref().map(|p| p.number),
                        pr_url: pr.as_ref().map(|p| p.url.clone()),
                        status: WorkerStatus::Done,
                        started_at: None,
                        ended_at: Some(timestamp()),
                        duration_s: None,
                        exit_code: None,
                        log_path: None,
                    };
                    let _ = store.save(&done_state);
                    println!(
                        "[{}] Recorded #{issue_num} as done (existing PR)",
                        timestamp()
                    );
                }

                if !plan.has_dispatch() {
                    println!(
                        "[{}] [{repo}] {} approved issue(s). No work to dispatch.",
                        timestamp(),
                        approved_issues.len()
                    );
                    continue;
                }

                // Build dispatcher
                let dispatcher = match WorkerDispatcher::new(
                    &sipag_dir,
                    &self.config.image,
                    self.config.timeout,
                    &self.config.work_label,
                ) {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!("[{}] Failed to create dispatcher: {e}", timestamp());
                        continue;
                    }
                };

                self.state = LoopState::Dispatching;

                // Execute: conflict fixes (highest priority)
                if !plan.dispatch_conflict_fixes.is_empty() {
                    println!(
                        "[{}] Found {} PR(s) with conflicts to fix: {:?}",
                        timestamp(),
                        plan.dispatch_conflict_fixes.len(),
                        plan.dispatch_conflict_fixes
                    );
                    self.execute_batch(
                        repo,
                        &plan.dispatch_conflict_fixes,
                        WorkItemKind::ConflictFix,
                        &dispatcher,
                        &mut running_conflict_fix_prs,
                    );
                }

                // Execute: PR iterations (before new issues)
                if !plan.dispatch_iterations.is_empty() {
                    println!(
                        "[{}] Found {} PR(s) needing iteration: {:?}",
                        timestamp(),
                        plan.dispatch_iterations.len(),
                        plan.dispatch_iterations
                    );
                    self.execute_batch(
                        repo,
                        &plan.dispatch_iterations,
                        WorkItemKind::Iteration,
                        &dispatcher,
                        &mut running_iteration_prs,
                    );
                }

                // Execute: new issues
                if !plan.dispatch_issues.is_empty() {
                    println!(
                        "[{}] Found {} new issue(s): {:?}",
                        timestamp(),
                        plan.dispatch_issues.len(),
                        plan.dispatch_issues
                    );
                    let mut dummy: HashSet<u64> = HashSet::new();
                    self.execute_batch(
                        repo,
                        &plan.dispatch_issues,
                        WorkItemKind::NewIssue,
                        &dispatcher,
                        &mut dummy,
                    );
                }

                println!("[{}] [{repo}] Cycle done.", timestamp());
            }

            self.cycle_count += 1;

            if self.config.once {
                if found_work {
                    println!("[{}] --once: cycle complete, exiting.", timestamp());
                } else {
                    println!("[{}] --once: no work found — exiting.", timestamp());
                }
                break;
            }

            println!(
                "[{}] Next poll in {}s...",
                timestamp(),
                self.config.poll_interval.as_secs()
            );
            std::thread::sleep(self.config.poll_interval);
        }

        self.state = LoopState::Stopped;
        Ok(())
    }

    /// Execute a batch of work items in chunks of `batch_size`, waiting for each chunk.
    fn execute_batch(
        &self,
        repo: &str,
        items: &[u64],
        kind: WorkItemKind,
        dispatcher: &WorkerDispatcher,
        running_set: &mut HashSet<u64>,
    ) {
        for chunk in items.chunks(self.config.batch_size) {
            println!("--- {} batch: {:?} ---", kind.label(), chunk);

            // Mark all in the chunk as running before spawning
            for &id in chunk {
                running_set.insert(id);
            }

            // Spawn threads for parallel execution
            let handles: Vec<_> = chunk
                .iter()
                .map(|&id| {
                    let repo_owned = repo.to_string();
                    let kind_copy = kind;
                    let d = dispatcher.clone();

                    std::thread::spawn(move || match kind_copy {
                        WorkItemKind::NewIssue => {
                            if let Err(e) = d.dispatch_issue(&repo_owned, id) {
                                eprintln!("[#{id}] Dispatch error: {e}");
                            }
                        }
                        WorkItemKind::Iteration => {
                            if let Err(e) = d.dispatch_pr_iteration(&repo_owned, id) {
                                eprintln!("[PR #{id}] Iteration error: {e}");
                            }
                        }
                        WorkItemKind::ConflictFix => {
                            if let Err(e) = d.dispatch_conflict_fix(&repo_owned, id) {
                                eprintln!("[PR #{id}] Conflict-fix error: {e}");
                            }
                        }
                    })
                })
                .collect();

            // Wait for all threads in the chunk
            for h in handles {
                let _ = h.join();
            }

            // Mark all as done in the running set
            for &id in chunk {
                running_set.remove(&id);
            }

            println!("--- {} batch complete ---", kind.label());
            println!();
        }
    }

    /// Current loop state.
    pub fn state(&self) -> &LoopState {
        &self.state
    }

    /// Number of completed poll cycles.
    pub fn cycle_count(&self) -> u64 {
        self.cycle_count
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Work item kind for dispatch.
#[derive(Debug, Clone, Copy)]
enum WorkItemKind {
    NewIssue,
    Iteration,
    ConflictFix,
}

impl WorkItemKind {
    fn label(self) -> &'static str {
        match self {
            Self::NewIssue => "Issue",
            Self::Iteration => "PR iteration",
            Self::ConflictFix => "Conflict fix",
        }
    }
}

fn timestamp() -> String {
    chrono::Utc::now().format("%H:%M:%S").to_string()
}

/// Build a map from issue_num → WorkerStatus for a specific repo.
fn build_status_map(workers: &[WorkerState], repo: &str) -> HashMap<u64, WorkerStatus> {
    workers
        .iter()
        .filter(|w| w.repo == repo)
        .map(|w| (w.issue_num, w.status))
        .collect()
}

/// Run the worker polling loop for the given repos.
///
/// Entry point called from the CLI `sipag work` subcommand.
pub fn run_worker_loop(repos: Vec<String>, sipag_dir: &Path, once: bool) -> Result<()> {
    let config = WorkerConfig::load(sipag_dir, once);
    let mut worker_loop = WorkerLoop::new(repos, config);
    worker_loop.run()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::cycle::IssueSnapshot;
    use std::collections::HashMap;
    use tempfile::TempDir;

    // ── Tests for WorkerLoop state machine (using mocks) ─────────────────────

    #[test]
    fn new_loop_starts_in_polling_state() {
        let dir = TempDir::new().unwrap();
        let config = WorkerConfig::load(dir.path(), false);
        let lp = WorkerLoop::new(vec!["owner/repo".to_string()], config);
        assert_eq!(lp.state(), &LoopState::Polling);
        assert_eq!(lp.cycle_count(), 0);
    }

    #[test]
    fn build_status_map_filters_by_repo() {
        use crate::worker::state::WorkerState;
        use crate::worker::status::WorkerStatus;

        let workers = vec![
            WorkerState {
                repo: "owner/repo1".to_string(),
                issue_num: 1,
                issue_title: "a".to_string(),
                branch: "b".to_string(),
                container_name: "c".to_string(),
                pr_num: None,
                pr_url: None,
                status: WorkerStatus::Running,
                started_at: None,
                ended_at: None,
                duration_s: None,
                exit_code: None,
                log_path: None,
            },
            WorkerState {
                repo: "owner/repo2".to_string(),
                issue_num: 2,
                status: WorkerStatus::Done,
                issue_title: "a".to_string(),
                branch: "b".to_string(),
                container_name: "c".to_string(),
                pr_num: None,
                pr_url: None,
                started_at: None,
                ended_at: None,
                duration_s: None,
                exit_code: None,
                log_path: None,
            },
        ];

        let map = build_status_map(&workers, "owner/repo1");
        assert_eq!(map.len(), 1);
        assert_eq!(map[&1], WorkerStatus::Running);
    }

    #[test]
    fn plan_cycle_integration_with_loop_data() {
        // Verify plan_cycle works correctly when called with loop-produced data.
        let issue_snapshots = vec![
            IssueSnapshot {
                issue_num: 1,
                has_existing_pr: false,
            },
            IssueSnapshot {
                issue_num: 2,
                has_existing_pr: true,
            }, // → record_as_done
        ];
        let worker_statuses: HashMap<u64, WorkerStatus> = HashMap::new();
        let prs_needing_iteration = vec![10u64];
        let conflicted_prs: Vec<u64> = vec![];
        let running_iter: Vec<u64> = vec![];
        let running_fix: Vec<u64> = vec![];

        let plan = plan_cycle(
            &issue_snapshots,
            &worker_statuses,
            &prs_needing_iteration,
            &conflicted_prs,
            &running_iter,
            &running_fix,
            1,
            false,
        );

        assert_eq!(plan.dispatch_issues, vec![1]);
        assert_eq!(plan.dispatch_iterations, vec![10]);
        assert_eq!(plan.record_as_done, vec![2]);
        assert!(plan.has_dispatch());
    }

    #[test]
    fn drain_signal_stops_dispatch() {
        let dir = TempDir::new().unwrap();
        let drain = DrainSignal::new(dir.path());
        drain.set().unwrap();

        // plan_cycle with drain=true returns empty plan
        let plan = plan_cycle(&[], &HashMap::new(), &[], &[], &[], &[], 1, true);
        assert!(!plan.has_dispatch());
    }

    #[test]
    fn worker_config_once_mode() {
        let dir = TempDir::new().unwrap();
        let config = WorkerConfig::load(dir.path(), true);
        assert!(config.once);
    }
}
