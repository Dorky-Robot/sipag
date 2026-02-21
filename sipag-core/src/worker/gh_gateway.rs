use anyhow::{Context, Result};
use std::process::{Command, Stdio};

use super::ports::{GitHubGateway, PrInfo};

/// Information about a GitHub issue.
#[derive(Debug, Clone)]
pub struct IssueInfo {
    pub number: u64,
    pub title: String,
    pub body: String,
}

/// Port for GitHub operations specific to the worker polling loop.
pub trait WorkerPoller {
    /// List open issue numbers with the given label.
    fn list_approved_issues(&self, repo: &str, label: &str) -> Result<Vec<u64>>;

    /// Find open PRs that have review feedback (CHANGES_REQUESTED or comments) after the last push.
    fn find_prs_needing_iteration(&self, repo: &str) -> Result<Vec<u64>>;

    /// Find open sipag/issue-* PRs with merge conflicts.
    fn find_conflicted_prs(&self, repo: &str) -> Result<Vec<u64>>;

    /// Check if an issue has an open or merged PR.
    fn has_pr_for_issue(&self, repo: &str, issue_num: u64) -> Result<bool>;

    /// Fetch issue title and body.
    fn get_issue(&self, repo: &str, issue_num: u64) -> Result<IssueInfo>;

    /// Auto-merge clean sipag PRs (MERGEABLE + CLEAN + not draft + no CHANGES_REQUESTED).
    fn auto_merge_clean_prs(&self, repo: &str) -> Result<()>;

    /// Reconcile: close issues whose PRs have been merged, clean up stale branches.
    fn reconcile_merged_prs(&self, repo: &str, work_label: &str) -> Result<()>;

    /// Fetch the current GitHub token via `gh auth token`.
    fn gh_token(&self) -> Result<String>;
}

/// Concrete adapter: calls the `gh` CLI for all GitHub operations.
///
/// Uses subprocess calls so sipag has no dependency on a GitHub API crate,
/// and inherits the user's existing `gh auth` session automatically.
pub struct GhCliGateway;

impl GhCliGateway {
    /// Run a `gh` command and capture stdout as a String.
    fn gh_output(args: &[&str]) -> Result<String> {
        let out = Command::new("gh")
            .args(args)
            .stderr(Stdio::null())
            .output()
            .with_context(|| format!("failed to run: gh {}", args.join(" ")))?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Run a `gh` command and return true if it succeeded.
    fn gh_status(args: &[&str]) -> bool {
        Command::new("gh")
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

impl GitHubGateway for GhCliGateway {
    fn find_pr_for_branch(&self, repo: &str, branch: &str) -> Result<Option<PrInfo>> {
        let out = GhCliGateway::gh_output(&[
            "pr",
            "list",
            "--repo",
            repo,
            "--head",
            branch,
            "--state",
            "all",
            "--json",
            "number,url",
            "-q",
            ".[0] | {number: .number, url: .url}",
        ])?;

        if out.is_empty() || out == "null" {
            return Ok(None);
        }

        let v: serde_json::Value = serde_json::from_str(&out).unwrap_or(serde_json::Value::Null);
        let number = v["number"].as_u64();
        let url = v["url"].as_str().map(str::to_string);

        match (number, url) {
            (Some(n), Some(u)) => Ok(Some(PrInfo { number: n, url: u })),
            _ => Ok(None),
        }
    }

    fn transition_label(
        &self,
        repo: &str,
        issue_num: u64,
        remove: Option<&str>,
        add: Option<&str>,
    ) -> Result<()> {
        let issue_str = issue_num.to_string();
        if let Some(label) = remove {
            // Ignore errors (issue may be closed)
            let _ = Command::new("gh")
                .args([
                    "issue",
                    "edit",
                    &issue_str,
                    "--repo",
                    repo,
                    "--remove-label",
                    label,
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        if let Some(label) = add {
            let _ = Command::new("gh")
                .args([
                    "issue",
                    "edit",
                    &issue_str,
                    "--repo",
                    repo,
                    "--add-label",
                    label,
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        Ok(())
    }
}

impl WorkerPoller for GhCliGateway {
    fn list_approved_issues(&self, repo: &str, label: &str) -> Result<Vec<u64>> {
        let out = GhCliGateway::gh_output(&[
            "issue",
            "list",
            "--repo",
            repo,
            "--state",
            "open",
            "--label",
            label,
            "--json",
            "number",
            "-q",
            ".[].number",
        ])?;

        let mut nums: Vec<u64> = out
            .lines()
            .filter(|l| !l.is_empty())
            .filter_map(|l| l.trim().parse().ok())
            .collect();
        nums.sort();
        Ok(nums)
    }

    fn find_prs_needing_iteration(&self, repo: &str) -> Result<Vec<u64>> {
        // Find open PRs with CHANGES_REQUESTED review or comments after the last push.
        // This mirrors the logic in lib/worker/github.sh worker_find_prs_needing_iteration().
        let jq = r#"
            .[] |
            (
                if (.commits | length) > 0
                then .commits[-1].committedDate
                else "1970-01-01T00:00:00Z"
                end
            ) as $last_push |
            select(
                ((.reviews // []) | map(select(.state == "CHANGES_REQUESTED" and .submittedAt > $last_push)) | length > 0) or
                ((.comments // []) | map(select(.createdAt > $last_push)) | length > 0)
            ) |
            .number
        "#;

        let out = GhCliGateway::gh_output(&[
            "pr",
            "list",
            "--repo",
            repo,
            "--state",
            "open",
            "--json",
            "number,reviews,commits,comments",
            "--jq",
            jq.trim(),
        ])?;

        let mut nums: Vec<u64> = out
            .lines()
            .filter(|l| !l.is_empty())
            .filter_map(|l| l.trim().parse().ok())
            .collect();
        nums.sort();
        Ok(nums)
    }

    fn find_conflicted_prs(&self, repo: &str) -> Result<Vec<u64>> {
        let jq = r#"
            .[] | select(
                (.headRefName | startswith("sipag/issue-")) and
                .mergeable == "CONFLICTING"
            ) | .number
        "#;

        let out = GhCliGateway::gh_output(&[
            "pr",
            "list",
            "--repo",
            repo,
            "--state",
            "open",
            "--json",
            "number,headRefName,mergeable",
            "--jq",
            jq.trim(),
        ])?;

        let mut nums: Vec<u64> = out
            .lines()
            .filter(|l| !l.is_empty())
            .filter_map(|l| l.trim().parse().ok())
            .collect();
        nums.sort();
        Ok(nums)
    }

    fn has_pr_for_issue(&self, repo: &str, issue_num: u64) -> Result<bool> {
        let search = format!("closes #{issue_num}");
        let jq = format!(
            r#".[] | select(
                (.body // "" | test("(closes|fixes|resolves) #{issue_num}\\b"; "i")) and
                (.state == "OPEN" or .mergedAt != null)
            )"#
        );

        let out = GhCliGateway::gh_output(&[
            "pr",
            "list",
            "--repo",
            repo,
            "--state",
            "all",
            "--search",
            &search,
            "--json",
            "number,body,state,mergedAt",
            "-q",
            &jq,
        ])?;

        Ok(!out.is_empty())
    }

    fn get_issue(&self, repo: &str, issue_num: u64) -> Result<IssueInfo> {
        let num_str = issue_num.to_string();
        let title = GhCliGateway::gh_output(&[
            "issue", "view", &num_str, "--repo", repo, "--json", "title", "-q", ".title",
        ])?;

        let body = GhCliGateway::gh_output(&[
            "issue", "view", &num_str, "--repo", repo, "--json", "body", "-q", ".body",
        ])?;

        Ok(IssueInfo {
            number: issue_num,
            title,
            body,
        })
    }

    fn auto_merge_clean_prs(&self, repo: &str) -> Result<()> {
        let jq = r#"
            .[] | select(
                (.headRefName | startswith("sipag/issue-")) and
                .mergeable == "MERGEABLE" and
                .mergeStateStatus == "CLEAN" and
                .isDraft == false and
                .reviewDecision != "CHANGES_REQUESTED"
            ) | .number
        "#;

        let out = GhCliGateway::gh_output(&[
            "pr",
            "list",
            "--repo",
            repo,
            "--state",
            "open",
            "--json",
            "number,headRefName,mergeable,mergeStateStatus,isDraft,reviewDecision",
            "--jq",
            jq.trim(),
        ])?;

        let candidates: Vec<u64> = out
            .lines()
            .filter(|l| !l.is_empty())
            .filter_map(|l| l.trim().parse().ok())
            .collect();

        if candidates.is_empty() {
            return Ok(());
        }

        println!("[auto-merge] {} candidate(s) to merge", candidates.len());

        for pr_num in candidates {
            let title = GhCliGateway::gh_output(&[
                "pr",
                "view",
                &pr_num.to_string(),
                "--repo",
                repo,
                "--json",
                "title",
                "-q",
                ".title",
            ])
            .unwrap_or_default();

            println!("[auto-merge] Merging PR #{pr_num}: {title}");
            if GhCliGateway::gh_status(&[
                "pr",
                "merge",
                &pr_num.to_string(),
                "--repo",
                repo,
                "--squash",
                "--delete-branch",
                "--subject",
                &title,
            ]) {
                println!("[auto-merge] Merged PR #{pr_num}");
            } else {
                println!("[auto-merge] Failed to merge PR #{pr_num} (may need manual review)");
            }
        }

        Ok(())
    }

    fn reconcile_merged_prs(&self, repo: &str, work_label: &str) -> Result<()> {
        // Find issues labeled "in-progress"
        let out = GhCliGateway::gh_output(&[
            "issue",
            "list",
            "--repo",
            repo,
            "--state",
            "open",
            "--label",
            "in-progress",
            "--json",
            "number",
            "-q",
            ".[].number",
        ])?;

        let in_progress: Vec<u64> = out
            .lines()
            .filter(|l| !l.is_empty())
            .filter_map(|l| l.trim().parse().ok())
            .collect();

        if in_progress.is_empty() {
            return Ok(());
        }

        for issue_num in in_progress {
            // Check timeline for a cross-referenced merged PR
            let jq = r#"
                .[] | select(.event == "cross-referenced") |
                select(.source.issue.pull_request.merged_at != null) |
                .source.issue.number
            "#;

            let merged_pr_out = GhCliGateway::gh_output(&[
                "api",
                &format!("repos/{repo}/issues/{issue_num}/timeline"),
                "--jq",
                jq.trim(),
            ])?;

            let merged_pr: Option<u64> = merged_pr_out
                .lines()
                .next()
                .and_then(|l| l.trim().parse().ok());

            let Some(merged_pr) = merged_pr else {
                continue;
            };

            let pr_title = GhCliGateway::gh_output(&[
                "pr",
                "view",
                &merged_pr.to_string(),
                "--repo",
                repo,
                "--json",
                "title",
                "-q",
                ".title",
            ])
            .unwrap_or_default();

            println!(
                "[reconcile] Closing #{issue_num} — resolved by merged PR #{merged_pr} ({pr_title})"
            );

            // Close the issue
            let _ = GhCliGateway::gh_status(&[
                "issue",
                "close",
                &issue_num.to_string(),
                "--repo",
                repo,
                "--comment",
                &format!("Closed by merged PR #{merged_pr}"),
            ]);

            // Restore label (remove in-progress, no replacement needed since done)
            let _ = self.transition_label(repo, issue_num, Some("in-progress"), None);
            let _ = work_label; // label is restored on failure only; success just closes

            // Delete the branch
            let branch = GhCliGateway::gh_output(&[
                "pr",
                "view",
                &merged_pr.to_string(),
                "--repo",
                repo,
                "--json",
                "headRefName",
                "-q",
                ".headRefName",
            ])
            .unwrap_or_default();

            if !branch.is_empty() {
                let _ = GhCliGateway::gh_status(&[
                    "api",
                    "-X",
                    "DELETE",
                    &format!("repos/{repo}/git/refs/heads/{branch}"),
                ]);
            }
        }

        Ok(())
    }

    fn gh_token(&self) -> Result<String> {
        GhCliGateway::gh_output(&["auth", "token"])
            .context("failed to get GitHub token via 'gh auth token'")
    }
}
