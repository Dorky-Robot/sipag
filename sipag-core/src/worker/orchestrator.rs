//! WorkerOrchestrator: coordinates issue runs via ports/adapters.
//!
//! Replaces the jq-based state mutations in `lib/worker/docker.sh` with
//! typed Rust: all state writes go through `StateStore::save`, all GitHub
//! interactions go through `GitHubGateway`, and all container operations
//! go through `ContainerRuntime`.

use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;

use super::container::{classify_exit, plan_issue_container, WorkerOutcome};
use super::ports::{ContainerConfig, ContainerRuntime, GitHubGateway, StateStore};
use super::prompt::build_issue_prompt;
use super::recovery::{recover_and_finalize, RecoveryOutcome};
use super::state::WorkerState;
use super::status::WorkerStatus;
use crate::task::naming::slugify;

/// Orchestrates the full lifecycle of a worker container for a GitHub issue.
///
/// Coordinates all side effects through ports:
/// - `G: GitHubGateway` — issue/PR queries and label transitions
/// - `S: StateStore` — worker state persistence
/// - `C: ContainerRuntime` — container execution and status checks
///
/// The bash equivalents of these methods are in `lib/worker/docker.sh`:
/// - `run_issue` ↔ `worker_run_issue()`
/// - `finalize_exited` ↔ `worker_finalize_exited()` + `worker_recover()`
pub struct WorkerOrchestrator<G, S, C> {
    github: G,
    store: S,
    containers: C,
    /// Issue label that marks an issue as ready for dispatch (e.g. "approved").
    work_label: String,
    /// Docker image to use for worker containers.
    image: String,
    /// Directory where container log files are written.
    log_dir: PathBuf,
    /// Template string for the issue worker prompt.
    /// Placeholders: `{{TITLE}}`, `{{BODY}}`, `{{BRANCH}}`, `{{ISSUE_NUM}}`.
    issue_prompt_template: String,
}

impl<G, S, C> WorkerOrchestrator<G, S, C>
where
    G: GitHubGateway,
    S: StateStore,
    C: ContainerRuntime,
{
    /// Create a new `WorkerOrchestrator`.
    pub fn new(
        github: G,
        store: S,
        containers: C,
        work_label: impl Into<String>,
        image: impl Into<String>,
        log_dir: PathBuf,
        issue_prompt_template: impl Into<String>,
    ) -> Self {
        Self {
            github,
            store,
            containers,
            work_label: work_label.into(),
            image: image.into(),
            log_dir,
            issue_prompt_template: issue_prompt_template.into(),
        }
    }

    /// Access the underlying state store (useful for inspection in tests).
    pub fn store(&self) -> &S {
        &self.store
    }

    /// Access the underlying GitHub gateway (useful for inspection in tests).
    pub fn github(&self) -> &G {
        &self.github
    }

    /// Run a new issue worker: write state, launch container, finalize.
    ///
    /// Blocks until the container exits.
    ///
    /// State transitions: (none) → `Enqueued` → `Running` → `Done | Failed`
    ///
    /// Mirrors `worker_run_issue()` in `lib/worker/docker.sh` but uses
    /// typed state transitions instead of jq + mktemp.
    pub fn run_issue(&self, repo: &str, issue_num: u64) -> Result<WorkerOutcome> {
        let repo_slug = repo.replace('/', "--");
        let container_name = format!("sipag-issue-{issue_num}");
        let log_path = self.log_dir.join(format!("{repo_slug}--{issue_num}.log"));

        // 1. Write enqueued state BEFORE any I/O — crash safety.
        //    Branch and title are empty at this stage; the container name
        //    depends only on issue_num so we can write it immediately.
        let enqueued = WorkerState {
            repo: repo.to_string(),
            issue_num,
            issue_title: String::new(),
            branch: String::new(),
            container_name: container_name.clone(),
            pr_num: None,
            pr_url: None,
            status: WorkerStatus::Enqueued,
            started_at: Some(utc_now()),
            ended_at: None,
            duration_s: None,
            exit_code: None,
            log_path: Some(log_path.clone()),
        };
        self.store.save(&enqueued)?;

        // 2. Transition label: work_label → "in-progress".
        //    Errors are ignored — a label failure must not abort the run.
        let _ = self.github.transition_label(
            repo,
            issue_num,
            Some(&self.work_label),
            Some("in-progress"),
        );

        // 3. Fetch issue title and body (fresh, just before container starts).
        let issue = self.github.get_issue(repo, issue_num)?;

        // 4. Compute container config — requires title for branch slug.
        let base_config =
            plan_issue_container(repo, issue_num, &issue.title, &self.image, &self.log_dir);

        // 5. Build prompt from template.
        let issue_prompt = build_issue_prompt(
            &self.issue_prompt_template,
            &issue.title,
            &issue.body,
            &base_config.branch,
            issue_num,
        );

        // 6. Assemble final container config with env vars.
        let mut env = HashMap::new();
        env.insert("PROMPT".to_string(), issue_prompt.prompt);
        env.insert("BRANCH".to_string(), base_config.branch.clone());
        env.insert("ISSUE_TITLE".to_string(), issue.title.clone());
        env.insert("PR_BODY".to_string(), issue_prompt.pr_body);

        let run_config = ContainerConfig {
            name: base_config.name.clone(),
            image: base_config.image.clone(),
            repo: base_config.repo.clone(),
            branch: base_config.branch.clone(),
            env,
            timeout: base_config.timeout,
            log_path: base_config.log_path.clone(),
        };

        // 7. Write running state (branch and title now known).
        let start_epoch = epoch_secs();
        let running = WorkerState {
            repo: repo.to_string(),
            issue_num,
            issue_title: issue.title.clone(),
            branch: base_config.branch.clone(),
            container_name: container_name.clone(),
            pr_num: None,
            pr_url: None,
            status: WorkerStatus::Running,
            started_at: Some(utc_now()),
            ended_at: None,
            duration_s: None,
            exit_code: None,
            log_path: Some(base_config.log_path.clone()),
        };
        self.store.save(&running)?;

        // 8. Run container — blocks until the container exits.
        let result = self.containers.run_container(&run_config)?;
        let duration_secs = epoch_secs() - start_epoch;

        // 9. Check for PR on the branch.
        let pr = self.github.find_pr_for_branch(repo, &base_config.branch)?;
        let outcome = classify_exit(result.exit_code, pr.is_some());

        // 10. Write final state and transition labels.
        let mut final_state = running;
        final_state.ended_at = Some(utc_now());
        final_state.duration_s = Some(duration_secs);
        final_state.exit_code = Some(i64::from(result.exit_code));

        match &outcome {
            WorkerOutcome::Done { .. } => {
                final_state.status = WorkerStatus::Done;
                if let Some(pr_info) = &pr {
                    final_state.pr_num = Some(pr_info.number);
                    final_state.pr_url = Some(pr_info.url.clone());
                }
                // Remove "in-progress"; PR's "Closes #N" closes the issue.
                let _ = self
                    .github
                    .transition_label(repo, issue_num, Some("in-progress"), None);
            }
            WorkerOutcome::Failed { .. } => {
                final_state.status = WorkerStatus::Failed;
                // Restore work_label so the next poll cycle re-dispatches.
                let _ = self.github.transition_label(
                    repo,
                    issue_num,
                    Some("in-progress"),
                    Some(&self.work_label),
                );
            }
        }
        self.store.save(&final_state)?;

        Ok(outcome)
    }

    /// Finalize all exited containers and update their state.
    ///
    /// Delegates to [`recover_and_finalize`]. Called at the top of each poll
    /// cycle — mirrors `worker_finalize_exited()` in `lib/worker/docker.sh`.
    pub fn finalize_exited(&self) -> Result<Vec<RecoveryOutcome>> {
        recover_and_finalize(
            &self.containers,
            &self.github,
            &self.store,
            &self.work_label,
        )
    }
}

/// Return current UTC time as an ISO-8601 string.
fn utc_now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Return current Unix epoch in seconds.
fn epoch_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Compute the branch name for an issue (pure, no side effects).
///
/// Used internally; also exported for callers that need the branch name
/// without building the full `ContainerConfig`.
pub fn issue_branch(issue_num: u64, title: &str) -> String {
    let slug: String = slugify(title).chars().take(50).collect();
    format!("sipag/issue-{issue_num}-{slug}")
}

#[cfg(test)]
mod tests {
    use super::super::ports::{ContainerConfig, ContainerResult, IssueInfo, PrInfo};
    use super::super::state::WorkerState;
    use super::super::status::WorkerStatus;
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use tempfile::TempDir;

    // ── Mock: ContainerRuntime ────────────────────────────────────────────────

    struct MockContainers {
        running: Vec<String>,
        run_result: ContainerResult,
    }

    impl ContainerRuntime for MockContainers {
        fn is_running(&self, name: &str) -> anyhow::Result<bool> {
            Ok(self.running.contains(&name.to_string()))
        }

        fn run_container(&self, _config: &ContainerConfig) -> anyhow::Result<ContainerResult> {
            Ok(self.run_result.clone())
        }
    }

    // ── Mock: GitHubGateway ───────────────────────────────────────────────────

    struct MockGitHub {
        issues: HashMap<u64, IssueInfo>,
        prs: HashMap<String, PrInfo>,
        label_calls: RefCell<Vec<LabelCall>>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct LabelCall {
        repo: String,
        issue: u64,
        remove: Option<String>,
        add: Option<String>,
    }

    impl MockGitHub {
        fn new() -> Self {
            Self {
                issues: HashMap::new(),
                prs: HashMap::new(),
                label_calls: RefCell::new(Vec::new()),
            }
        }

        fn with_issue(mut self, num: u64, title: &str, body: &str) -> Self {
            self.issues.insert(
                num,
                IssueInfo {
                    title: title.to_string(),
                    body: body.to_string(),
                },
            );
            self
        }

        fn with_pr(mut self, branch: &str, number: u64) -> Self {
            self.prs.insert(
                branch.to_string(),
                PrInfo {
                    number,
                    url: format!("https://github.com/test/repo/pull/{number}"),
                },
            );
            self
        }
    }

    impl GitHubGateway for MockGitHub {
        fn find_pr_for_branch(&self, _repo: &str, branch: &str) -> anyhow::Result<Option<PrInfo>> {
            Ok(self.prs.get(branch).cloned())
        }

        fn transition_label(
            &self,
            repo: &str,
            issue: u64,
            remove: Option<&str>,
            add: Option<&str>,
        ) -> anyhow::Result<()> {
            self.label_calls.borrow_mut().push(LabelCall {
                repo: repo.to_string(),
                issue,
                remove: remove.map(|s| s.to_string()),
                add: add.map(|s| s.to_string()),
            });
            Ok(())
        }

        fn get_issue(&self, _repo: &str, issue_num: u64) -> anyhow::Result<IssueInfo> {
            self.issues
                .get(&issue_num)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("issue #{issue_num} not found in mock"))
        }
    }

    // ── Mock: StateStore ──────────────────────────────────────────────────────

    struct MockStore {
        workers: RefCell<Vec<WorkerState>>,
    }

    impl MockStore {
        fn new() -> Self {
            Self {
                workers: RefCell::new(Vec::new()),
            }
        }

        fn get(&self, issue_num: u64) -> Option<WorkerState> {
            self.workers
                .borrow()
                .iter()
                .find(|w| w.issue_num == issue_num)
                .cloned()
        }
    }

    impl StateStore for MockStore {
        fn load(&self, _repo_slug: &str, issue_num: u64) -> anyhow::Result<Option<WorkerState>> {
            Ok(self
                .workers
                .borrow()
                .iter()
                .find(|w| w.issue_num == issue_num)
                .cloned())
        }

        fn save(&self, state: &WorkerState) -> anyhow::Result<()> {
            let mut workers = self.workers.borrow_mut();
            if let Some(existing) = workers.iter_mut().find(|w| w.issue_num == state.issue_num) {
                *existing = state.clone();
            } else {
                workers.push(state.clone());
            }
            Ok(())
        }

        fn list_active(&self) -> anyhow::Result<Vec<WorkerState>> {
            Ok(self
                .workers
                .borrow()
                .iter()
                .filter(|w| w.status.is_active())
                .cloned()
                .collect())
        }
    }

    // ── Helper ────────────────────────────────────────────────────────────────

    fn make_orchestrator(
        github: MockGitHub,
        store: MockStore,
        containers: MockContainers,
        log_dir: &std::path::Path,
    ) -> WorkerOrchestrator<MockGitHub, MockStore, MockContainers> {
        WorkerOrchestrator::new(
            github,
            store,
            containers,
            "approved",
            "sipag-worker:test",
            log_dir.to_path_buf(),
            "Task: {{TITLE}}\n\n{{BODY}}\n\nBranch: {{BRANCH}}\nIssue: #{{ISSUE_NUM}}",
        )
    }

    fn success_containers() -> MockContainers {
        MockContainers {
            running: vec![],
            run_result: ContainerResult {
                exit_code: 0,
                duration_secs: 60,
            },
        }
    }

    fn failure_containers() -> MockContainers {
        MockContainers {
            running: vec![],
            run_result: ContainerResult {
                exit_code: 1,
                duration_secs: 10,
            },
        }
    }

    // ── Tests: run_issue outcomes ─────────────────────────────────────────────

    #[test]
    fn run_issue_success_with_pr_returns_done() {
        let dir = TempDir::new().unwrap();
        let github = MockGitHub::new()
            .with_issue(42, "Fix the bug", "Bug description")
            .with_pr("sipag/issue-42-fix-the-bug", 100);
        let orchestrator =
            make_orchestrator(github, MockStore::new(), success_containers(), dir.path());

        let outcome = orchestrator.run_issue("owner/repo", 42).unwrap();

        assert!(matches!(outcome, WorkerOutcome::Done { .. }));
    }

    #[test]
    fn run_issue_exit_failure_returns_failed() {
        let dir = TempDir::new().unwrap();
        let github = MockGitHub::new().with_issue(42, "Fix the bug", "Bug description");
        let orchestrator =
            make_orchestrator(github, MockStore::new(), failure_containers(), dir.path());

        let outcome = orchestrator.run_issue("owner/repo", 42).unwrap();

        assert_eq!(outcome, WorkerOutcome::Failed { exit_code: 1 });
    }

    #[test]
    fn run_issue_exit_0_without_pr_returns_failed() {
        let dir = TempDir::new().unwrap();
        // No PR registered — Claude exited 0 but forgot to create a PR.
        let github = MockGitHub::new().with_issue(42, "Fix the bug", "Bug description");
        let orchestrator =
            make_orchestrator(github, MockStore::new(), success_containers(), dir.path());

        let outcome = orchestrator.run_issue("owner/repo", 42).unwrap();

        assert_eq!(outcome, WorkerOutcome::Failed { exit_code: 0 });
    }

    // ── Tests: state transitions ──────────────────────────────────────────────

    #[test]
    fn run_issue_done_stores_pr_info_in_state() {
        let dir = TempDir::new().unwrap();
        let github = MockGitHub::new()
            .with_issue(42, "Fix the bug", "Bug description")
            .with_pr("sipag/issue-42-fix-the-bug", 100);
        let orchestrator =
            make_orchestrator(github, MockStore::new(), success_containers(), dir.path());

        orchestrator.run_issue("owner/repo", 42).unwrap();

        let state = orchestrator.store().get(42).unwrap();
        assert_eq!(state.status, WorkerStatus::Done);
        assert_eq!(state.pr_num, Some(100));
        assert!(state.pr_url.is_some());
    }

    #[test]
    fn run_issue_failed_marks_state_as_failed() {
        let dir = TempDir::new().unwrap();
        let github = MockGitHub::new().with_issue(42, "Fix the bug", "Bug description");
        let orchestrator =
            make_orchestrator(github, MockStore::new(), failure_containers(), dir.path());

        orchestrator.run_issue("owner/repo", 42).unwrap();

        let state = orchestrator.store().get(42).unwrap();
        assert_eq!(state.status, WorkerStatus::Failed);
        assert_eq!(state.exit_code, Some(1));
    }

    #[test]
    fn run_issue_stores_issue_title_in_final_state() {
        let dir = TempDir::new().unwrap();
        let github = MockGitHub::new().with_issue(42, "Fix the bug", "Bug description");
        let orchestrator =
            make_orchestrator(github, MockStore::new(), failure_containers(), dir.path());

        orchestrator.run_issue("owner/repo", 42).unwrap();

        let state = orchestrator.store().get(42).unwrap();
        assert_eq!(state.issue_title, "Fix the bug");
    }

    #[test]
    fn run_issue_state_has_duration() {
        let dir = TempDir::new().unwrap();
        let github = MockGitHub::new().with_issue(42, "Fix the bug", "Bug description");
        let orchestrator =
            make_orchestrator(github, MockStore::new(), failure_containers(), dir.path());

        orchestrator.run_issue("owner/repo", 42).unwrap();

        let state = orchestrator.store().get(42).unwrap();
        assert!(state.duration_s.is_some());
        assert!(state.ended_at.is_some());
    }

    // ── Tests: label transitions ──────────────────────────────────────────────

    #[test]
    fn run_issue_transitions_label_to_in_progress_at_start() {
        let dir = TempDir::new().unwrap();
        let github = MockGitHub::new().with_issue(42, "Fix the bug", "Bug description");
        let orchestrator =
            make_orchestrator(github, MockStore::new(), failure_containers(), dir.path());

        orchestrator.run_issue("owner/repo", 42).unwrap();

        let calls = orchestrator.github().label_calls.borrow();
        assert!(calls.iter().any(|c| {
            c.remove == Some("approved".to_string()) && c.add == Some("in-progress".to_string())
        }));
    }

    #[test]
    fn run_issue_done_removes_in_progress_label() {
        let dir = TempDir::new().unwrap();
        let github = MockGitHub::new()
            .with_issue(42, "Fix the bug", "Bug description")
            .with_pr("sipag/issue-42-fix-the-bug", 100);
        let orchestrator =
            make_orchestrator(github, MockStore::new(), success_containers(), dir.path());

        orchestrator.run_issue("owner/repo", 42).unwrap();

        let calls = orchestrator.github().label_calls.borrow();
        assert!(calls
            .iter()
            .any(|c| { c.remove == Some("in-progress".to_string()) && c.add.is_none() }));
    }

    #[test]
    fn run_issue_failed_restores_work_label() {
        let dir = TempDir::new().unwrap();
        let github = MockGitHub::new().with_issue(42, "Fix the bug", "Bug description");
        let orchestrator =
            make_orchestrator(github, MockStore::new(), failure_containers(), dir.path());

        orchestrator.run_issue("owner/repo", 42).unwrap();

        let calls = orchestrator.github().label_calls.borrow();
        assert!(calls.iter().any(|c| {
            c.remove == Some("in-progress".to_string()) && c.add == Some("approved".to_string())
        }));
    }

    #[test]
    fn custom_work_label_restored_on_failure() {
        let dir = TempDir::new().unwrap();
        let github = MockGitHub::new().with_issue(42, "Fix the bug", "Bug description");
        let store = MockStore::new();
        let orchestrator = WorkerOrchestrator::new(
            github,
            store,
            failure_containers(),
            "ready-to-work",
            "sipag-worker:test",
            dir.path().to_path_buf(),
            "{{TITLE}}",
        );

        orchestrator.run_issue("owner/repo", 42).unwrap();

        let calls = orchestrator.github().label_calls.borrow();
        assert!(calls
            .iter()
            .any(|c| c.add == Some("ready-to-work".to_string())));
    }

    // ── Tests: finalize_exited ────────────────────────────────────────────────

    #[test]
    fn finalize_exited_with_empty_store_returns_empty() {
        let dir = TempDir::new().unwrap();
        let orchestrator = make_orchestrator(
            MockGitHub::new(),
            MockStore::new(),
            success_containers(),
            dir.path(),
        );

        let outcomes = orchestrator.finalize_exited().unwrap();
        assert!(outcomes.is_empty());
    }

    // ── Tests: issue_branch helper ────────────────────────────────────────────

    #[test]
    fn issue_branch_produces_correct_format() {
        assert_eq!(
            issue_branch(42, "Fix the bug"),
            "sipag/issue-42-fix-the-bug"
        );
    }

    #[test]
    fn issue_branch_slug_truncated_to_50_chars() {
        let long = "This is a very long issue title that exceeds fifty characters easily";
        let branch = issue_branch(1, long);
        let slug = branch.strip_prefix("sipag/issue-1-").unwrap();
        assert!(slug.len() <= 50);
    }
}
