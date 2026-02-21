//! Auto-merge service for clean sipag PRs.
//!
//! Ports `lib/worker/merge.sh` to Rust. The shell function `worker_auto_merge`
//! is replaced by [`AutoMergeService::merge_clean_prs`]; jq-based PR filtering
//! is replaced by the pure function [`is_auto_mergeable`].

use anyhow::Result;

use super::ports::{GitHubGateway, MergeState, Mergeable, PrMergeCandidate, ReviewDecision};

/// Outcome of attempting to auto-merge a single PR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    /// PR was successfully merged.
    Merged { number: u64, title: String },
    /// Merge attempt failed (e.g. race condition, API error).
    Failed { number: u64, title: String },
}

/// Pure function: is this PR safe to auto-merge?
///
/// A PR is auto-mergeable if ALL of the following hold:
///   - Branch starts with `sipag/issue-` (worker-created branch)
///   - `mergeable == Mergeable` (no conflicts)
///   - `merge_state == Clean` (all checks pass, no blocking reviews)
///   - Not a draft (worker finished pushing commits)
///   - `review_decision` is not `ChangesRequested`
pub fn is_auto_mergeable(pr: &PrMergeCandidate) -> bool {
    pr.branch.starts_with("sipag/issue-")
        && pr.mergeable == Mergeable::Mergeable
        && pr.merge_state == MergeState::Clean
        && !pr.is_draft
        && pr.review_decision != ReviewDecision::ChangesRequested
}

/// Service that merges clean, approved sipag PRs.
///
/// Wraps auto-merge logic and delegates all GitHub operations to the
/// [`GitHubGateway`] port — enabling unit testing via mock injection.
pub struct AutoMergeService<G: GitHubGateway> {
    github: G,
}

impl<G: GitHubGateway> AutoMergeService<G> {
    /// Create a new `AutoMergeService` backed by the given gateway.
    pub fn new(github: G) -> Self {
        Self { github }
    }

    /// Merge all clean sipag PRs for the given repo.
    ///
    /// Lists open PRs, filters to those satisfying [`is_auto_mergeable`],
    /// merges each one with squash strategy, and fires the `on-pr-merged`
    /// hook on success. Hook failures are ignored (best-effort).
    ///
    /// Returns one [`MergeOutcome`] per mergeable candidate, in list order.
    pub fn merge_clean_prs(&self, repo: &str) -> Result<Vec<MergeOutcome>> {
        let candidates = self.github.list_mergeable_prs(repo)?;
        let mut outcomes = Vec::new();

        for pr in candidates.iter().filter(|pr| is_auto_mergeable(pr)) {
            match self.github.merge_pr(repo, pr.number, &pr.title) {
                Ok(()) => {
                    let num_str = pr.number.to_string();
                    let _ = self.github.fire_hook(
                        "on-pr-merged",
                        &[
                            ("SIPAG_EVENT", "pr.auto-merged"),
                            ("SIPAG_PR_NUM", &num_str),
                            ("SIPAG_PR_TITLE", &pr.title),
                        ],
                    );
                    outcomes.push(MergeOutcome::Merged {
                        number: pr.number,
                        title: pr.title.clone(),
                    });
                }
                Err(_) => {
                    outcomes.push(MergeOutcome::Failed {
                        number: pr.number,
                        title: pr.title.clone(),
                    });
                }
            }
        }

        Ok(outcomes)
    }
}

#[cfg(test)]
mod tests {
    use super::super::ports::PrInfo;
    use super::*;
    use std::cell::RefCell;

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn make_pr(
        number: u64,
        branch: &str,
        mergeable: Mergeable,
        merge_state: MergeState,
        is_draft: bool,
        review_decision: ReviewDecision,
    ) -> PrMergeCandidate {
        PrMergeCandidate {
            number,
            title: format!("PR #{}", number),
            branch: branch.to_string(),
            mergeable,
            merge_state,
            is_draft,
            review_decision,
        }
    }

    fn clean_sipag_pr(number: u64) -> PrMergeCandidate {
        make_pr(
            number,
            &format!("sipag/issue-{}-fix", number),
            Mergeable::Mergeable,
            MergeState::Clean,
            false,
            ReviewDecision::None,
        )
    }

    // ── is_auto_mergeable ────────────────────────────────────────────────────

    #[test]
    fn clean_sipag_pr_is_mergeable() {
        assert!(is_auto_mergeable(&clean_sipag_pr(1)));
    }

    #[test]
    fn non_sipag_branch_rejected() {
        let pr = make_pr(
            1,
            "feature/my-feature",
            Mergeable::Mergeable,
            MergeState::Clean,
            false,
            ReviewDecision::None,
        );
        assert!(!is_auto_mergeable(&pr));
    }

    #[test]
    fn branch_prefix_must_be_exact() {
        // "not-sipag/issue-" does not start with "sipag/issue-"
        let pr = make_pr(
            1,
            "not-sipag/issue-1-fix",
            Mergeable::Mergeable,
            MergeState::Clean,
            false,
            ReviewDecision::None,
        );
        assert!(!is_auto_mergeable(&pr));
    }

    #[test]
    fn conflicting_pr_rejected() {
        let pr = make_pr(
            1,
            "sipag/issue-1-fix",
            Mergeable::Conflicting,
            MergeState::Clean,
            false,
            ReviewDecision::None,
        );
        assert!(!is_auto_mergeable(&pr));
    }

    #[test]
    fn unknown_mergeable_rejected() {
        let pr = make_pr(
            1,
            "sipag/issue-1-fix",
            Mergeable::Unknown,
            MergeState::Clean,
            false,
            ReviewDecision::None,
        );
        assert!(!is_auto_mergeable(&pr));
    }

    #[test]
    fn dirty_merge_state_rejected() {
        let pr = make_pr(
            1,
            "sipag/issue-1-fix",
            Mergeable::Mergeable,
            MergeState::Dirty,
            false,
            ReviewDecision::None,
        );
        assert!(!is_auto_mergeable(&pr));
    }

    #[test]
    fn blocked_merge_state_rejected() {
        let pr = make_pr(
            1,
            "sipag/issue-1-fix",
            Mergeable::Mergeable,
            MergeState::Blocked,
            false,
            ReviewDecision::None,
        );
        assert!(!is_auto_mergeable(&pr));
    }

    #[test]
    fn unstable_merge_state_rejected() {
        let pr = make_pr(
            1,
            "sipag/issue-1-fix",
            Mergeable::Mergeable,
            MergeState::Unstable,
            false,
            ReviewDecision::None,
        );
        assert!(!is_auto_mergeable(&pr));
    }

    #[test]
    fn behind_merge_state_rejected() {
        let pr = make_pr(
            1,
            "sipag/issue-1-fix",
            Mergeable::Mergeable,
            MergeState::Behind,
            false,
            ReviewDecision::None,
        );
        assert!(!is_auto_mergeable(&pr));
    }

    #[test]
    fn unknown_merge_state_rejected() {
        let pr = make_pr(
            1,
            "sipag/issue-1-fix",
            Mergeable::Mergeable,
            MergeState::Unknown,
            false,
            ReviewDecision::None,
        );
        assert!(!is_auto_mergeable(&pr));
    }

    #[test]
    fn draft_pr_rejected() {
        let pr = make_pr(
            1,
            "sipag/issue-1-fix",
            Mergeable::Mergeable,
            MergeState::Clean,
            true,
            ReviewDecision::None,
        );
        assert!(!is_auto_mergeable(&pr));
    }

    #[test]
    fn changes_requested_rejected() {
        let pr = make_pr(
            1,
            "sipag/issue-1-fix",
            Mergeable::Mergeable,
            MergeState::Clean,
            false,
            ReviewDecision::ChangesRequested,
        );
        assert!(!is_auto_mergeable(&pr));
    }

    #[test]
    fn approved_pr_is_mergeable() {
        let pr = make_pr(
            1,
            "sipag/issue-1-fix",
            Mergeable::Mergeable,
            MergeState::Clean,
            false,
            ReviewDecision::Approved,
        );
        assert!(is_auto_mergeable(&pr));
    }

    #[test]
    fn review_required_pr_is_mergeable() {
        // ReviewRequired means no review yet — not explicitly blocked.
        let pr = make_pr(
            1,
            "sipag/issue-1-fix",
            Mergeable::Mergeable,
            MergeState::Clean,
            false,
            ReviewDecision::ReviewRequired,
        );
        assert!(is_auto_mergeable(&pr));
    }

    // ── Exhaustiveness: all (mergeable, merge_state) combos ──────────────────

    #[test]
    fn all_mergeable_and_state_combinations_handled() {
        let mergeables = [
            Mergeable::Mergeable,
            Mergeable::Conflicting,
            Mergeable::Unknown,
        ];
        let states = [
            MergeState::Clean,
            MergeState::Dirty,
            MergeState::Blocked,
            MergeState::Unstable,
            MergeState::Behind,
            MergeState::Unknown,
        ];
        let decisions = [
            ReviewDecision::Approved,
            ReviewDecision::ChangesRequested,
            ReviewDecision::ReviewRequired,
            ReviewDecision::None,
        ];
        for mergeable in &mergeables {
            for state in &states {
                for decision in &decisions {
                    for is_draft in [true, false] {
                        let pr = make_pr(
                            1,
                            "sipag/issue-1-fix",
                            mergeable.clone(),
                            state.clone(),
                            is_draft,
                            decision.clone(),
                        );
                        // Must not panic — all combinations are handled.
                        let _ = is_auto_mergeable(&pr);
                    }
                }
            }
        }
    }

    // ── Mock ─────────────────────────────────────────────────────────────────

    type HookEnv = Vec<(String, String)>;

    struct MockGitHub {
        prs: Vec<PrMergeCandidate>,
        merge_should_fail: bool,
        merge_calls: RefCell<Vec<(String, u64, String)>>,
        hook_calls: RefCell<Vec<(String, HookEnv)>>,
    }

    impl MockGitHub {
        fn new(prs: Vec<PrMergeCandidate>) -> Self {
            Self {
                prs,
                merge_should_fail: false,
                merge_calls: RefCell::new(Vec::new()),
                hook_calls: RefCell::new(Vec::new()),
            }
        }

        fn with_failing_merges(mut self) -> Self {
            self.merge_should_fail = true;
            self
        }
    }

    impl GitHubGateway for MockGitHub {
        fn find_pr_for_branch(&self, _repo: &str, _branch: &str) -> Result<Option<PrInfo>> {
            Ok(None)
        }

        fn transition_label(
            &self,
            _repo: &str,
            _issue_num: u64,
            _remove: Option<&str>,
            _add: Option<&str>,
        ) -> Result<()> {
            Ok(())
        }

        fn list_mergeable_prs(&self, _repo: &str) -> Result<Vec<PrMergeCandidate>> {
            Ok(self.prs.clone())
        }

        fn merge_pr(&self, repo: &str, pr_num: u64, title: &str) -> Result<()> {
            self.merge_calls
                .borrow_mut()
                .push((repo.to_string(), pr_num, title.to_string()));
            if self.merge_should_fail {
                Err(anyhow::anyhow!("merge failed"))
            } else {
                Ok(())
            }
        }

        fn fire_hook(&self, hook_name: &str, env: &[(&str, &str)]) -> Result<()> {
            let env_owned: Vec<(String, String)> = env
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            self.hook_calls
                .borrow_mut()
                .push((hook_name.to_string(), env_owned));
            Ok(())
        }
    }

    // ── AutoMergeService ─────────────────────────────────────────────────────

    #[test]
    fn no_candidates_returns_empty() {
        let github = MockGitHub::new(vec![]);
        let service = AutoMergeService::new(github);
        let outcomes = service.merge_clean_prs("owner/repo").unwrap();
        assert!(outcomes.is_empty());
    }

    #[test]
    fn clean_pr_is_merged_and_reported() {
        let github = MockGitHub::new(vec![clean_sipag_pr(42)]);
        let service = AutoMergeService::new(github);
        let outcomes = service.merge_clean_prs("owner/repo").unwrap();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(
            outcomes[0],
            MergeOutcome::Merged {
                number: 42,
                title: "PR #42".to_string(),
            }
        );
    }

    #[test]
    fn non_mergeable_pr_skipped() {
        let pr = make_pr(
            1,
            "sipag/issue-1-fix",
            Mergeable::Conflicting,
            MergeState::Clean,
            false,
            ReviewDecision::None,
        );
        let github = MockGitHub::new(vec![pr]);
        let service = AutoMergeService::new(github);
        let outcomes = service.merge_clean_prs("owner/repo").unwrap();
        assert!(outcomes.is_empty());
        assert!(service.github.merge_calls.borrow().is_empty());
    }

    #[test]
    fn draft_pr_skipped() {
        let pr = make_pr(
            1,
            "sipag/issue-1-fix",
            Mergeable::Mergeable,
            MergeState::Clean,
            true, // is_draft
            ReviewDecision::None,
        );
        let github = MockGitHub::new(vec![pr]);
        let service = AutoMergeService::new(github);
        let outcomes = service.merge_clean_prs("owner/repo").unwrap();
        assert!(outcomes.is_empty());
    }

    #[test]
    fn failed_merge_recorded_as_failed() {
        let github = MockGitHub::new(vec![clean_sipag_pr(42)]).with_failing_merges();
        let service = AutoMergeService::new(github);
        let outcomes = service.merge_clean_prs("owner/repo").unwrap();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(
            outcomes[0],
            MergeOutcome::Failed {
                number: 42,
                title: "PR #42".to_string(),
            }
        );
    }

    #[test]
    fn hook_fired_on_successful_merge() {
        let github = MockGitHub::new(vec![clean_sipag_pr(42)]);
        let service = AutoMergeService::new(github);
        let _ = service.merge_clean_prs("owner/repo").unwrap();

        let hook_calls = service.github.hook_calls.borrow();
        assert_eq!(hook_calls.len(), 1);
        assert_eq!(hook_calls[0].0, "on-pr-merged");

        let env = &hook_calls[0].1;
        assert!(env
            .iter()
            .any(|(k, v)| k == "SIPAG_EVENT" && v == "pr.auto-merged"));
        assert!(env.iter().any(|(k, v)| k == "SIPAG_PR_NUM" && v == "42"));
        assert!(env
            .iter()
            .any(|(k, v)| k == "SIPAG_PR_TITLE" && v == "PR #42"));
    }

    #[test]
    fn hook_not_fired_on_failed_merge() {
        let github = MockGitHub::new(vec![clean_sipag_pr(42)]).with_failing_merges();
        let service = AutoMergeService::new(github);
        let _ = service.merge_clean_prs("owner/repo").unwrap();

        let hook_calls = service.github.hook_calls.borrow();
        assert!(hook_calls.is_empty());
    }

    #[test]
    fn hook_not_fired_for_skipped_prs() {
        let pr = make_pr(
            1,
            "feature/unrelated",
            Mergeable::Mergeable,
            MergeState::Clean,
            false,
            ReviewDecision::None,
        );
        let github = MockGitHub::new(vec![pr]);
        let service = AutoMergeService::new(github);
        let _ = service.merge_clean_prs("owner/repo").unwrap();

        assert!(service.github.hook_calls.borrow().is_empty());
    }

    #[test]
    fn multiple_prs_some_merged_some_skipped() {
        let prs = vec![
            clean_sipag_pr(1),
            make_pr(
                2,
                "sipag/issue-2-fix",
                Mergeable::Conflicting,
                MergeState::Clean,
                false,
                ReviewDecision::None,
            ),
            clean_sipag_pr(3),
        ];
        let github = MockGitHub::new(prs);
        let service = AutoMergeService::new(github);
        let outcomes = service.merge_clean_prs("owner/repo").unwrap();

        assert_eq!(outcomes.len(), 2);
        assert_eq!(
            outcomes[0],
            MergeOutcome::Merged {
                number: 1,
                title: "PR #1".to_string(),
            }
        );
        assert_eq!(
            outcomes[1],
            MergeOutcome::Merged {
                number: 3,
                title: "PR #3".to_string(),
            }
        );
    }

    #[test]
    fn merge_called_with_correct_args() {
        let github = MockGitHub::new(vec![clean_sipag_pr(42)]);
        let service = AutoMergeService::new(github);
        let _ = service.merge_clean_prs("owner/repo").unwrap();

        let calls = service.github.merge_calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "owner/repo");
        assert_eq!(calls[0].1, 42);
        assert_eq!(calls[0].2, "PR #42");
    }

    #[test]
    fn changes_requested_pr_skipped_no_merge_call() {
        let pr = make_pr(
            1,
            "sipag/issue-1-fix",
            Mergeable::Mergeable,
            MergeState::Clean,
            false,
            ReviewDecision::ChangesRequested,
        );
        let github = MockGitHub::new(vec![pr]);
        let service = AutoMergeService::new(github);
        let outcomes = service.merge_clean_prs("owner/repo").unwrap();

        assert!(outcomes.is_empty());
        assert!(service.github.merge_calls.borrow().is_empty());
    }

    #[test]
    fn hook_fired_once_per_merged_pr() {
        let github = MockGitHub::new(vec![clean_sipag_pr(1), clean_sipag_pr(2)]);
        let service = AutoMergeService::new(github);
        let outcomes = service.merge_clean_prs("owner/repo").unwrap();

        assert_eq!(outcomes.len(), 2);
        assert_eq!(service.github.hook_calls.borrow().len(), 2);
    }
}
