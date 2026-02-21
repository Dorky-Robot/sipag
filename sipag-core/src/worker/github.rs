//! GitHub operations via the `gh` CLI.
//!
//! Mirrors `lib/worker/github.sh` and implements `GitHubGateway` for
//! production use with the `gh` CLI tool.

use anyhow::{Context, Result};
use std::process::{Command, Stdio};

use super::github_gateway::GhCliGateway;
use super::ports::{GitHubGateway, IssueInfo, PrInfo, PrState, TimelineEvent};

/// Production `GitHubGateway` that delegates to the `gh` CLI.
pub struct GhGateway;

impl Default for GhGateway {
    fn default() -> Self {
        Self::new()
    }
}

impl GhGateway {
    pub fn new() -> Self {
        Self
    }
}

impl GitHubGateway for GhGateway {
    fn find_pr_for_branch(&self, repo: &str, branch: &str) -> Result<Option<PrInfo>> {
        GhCliGateway.find_pr_for_branch(repo, branch)
    }

    fn find_pr_for_issue(&self, repo: &str, issue_num: u64) -> Result<Option<PrInfo>> {
        GhCliGateway.find_pr_for_issue(repo, issue_num)
    }

    fn find_open_pr_for_issue(&self, repo: &str, issue_num: u64) -> Result<Option<PrInfo>> {
        GhCliGateway.find_open_pr_for_issue(repo, issue_num)
    }

    fn find_prs_needing_iteration(&self, repo: &str) -> Result<Vec<u64>> {
        GhCliGateway.find_prs_needing_iteration(repo)
    }

    fn find_conflicted_prs(&self, repo: &str) -> Result<Vec<PrInfo>> {
        GhCliGateway.find_conflicted_prs(repo)
    }

    fn issue_is_open(&self, repo: &str, issue_num: u64) -> Result<bool> {
        GhCliGateway.issue_is_open(repo, issue_num)
    }

    fn get_issue_info(&self, repo: &str, issue_num: u64) -> Result<Option<IssueInfo>> {
        GhCliGateway.get_issue_info(repo, issue_num)
    }

    fn list_issues_with_label(&self, repo: &str, label: &str) -> Result<Vec<u64>> {
        GhCliGateway.list_issues_with_label(repo, label)
    }

    fn get_issue_timeline(&self, repo: &str, issue_num: u64) -> Result<Vec<TimelineEvent>> {
        GhCliGateway.get_issue_timeline(repo, issue_num)
    }

    fn close_issue(&self, repo: &str, issue_num: u64, comment: &str) -> Result<()> {
        GhCliGateway.close_issue(repo, issue_num, comment)
    }

    fn transition_label(
        &self,
        repo: &str,
        issue_num: u64,
        remove: Option<&str>,
        add: Option<&str>,
    ) -> Result<()> {
        GhCliGateway.transition_label(repo, issue_num, remove, add)
    }

    fn get_pr_info(&self, repo: &str, pr_num: u64) -> Result<Option<PrInfo>> {
        GhCliGateway.get_pr_info(repo, pr_num)
    }

    fn get_open_prs_for_branch(&self, repo: &str, branch: &str) -> Result<Vec<PrInfo>> {
        GhCliGateway.get_open_prs_for_branch(repo, branch)
    }

    fn get_merged_prs_for_branch(&self, repo: &str, branch: &str) -> Result<Vec<PrInfo>> {
        GhCliGateway.get_merged_prs_for_branch(repo, branch)
    }

    fn list_branches_with_prefix(&self, repo: &str, prefix: &str) -> Result<Vec<String>> {
        GhCliGateway.list_branches_with_prefix(repo, prefix)
    }

    fn branch_ahead_by(&self, repo: &str, base: &str, head: &str) -> Result<u64> {
        GhCliGateway.branch_ahead_by(repo, base, head)
    }

    fn create_pr(&self, repo: &str, branch: &str, title: &str, body: &str) -> Result<()> {
        GhCliGateway.create_pr(repo, branch, title, body)
    }

    fn delete_branch(&self, repo: &str, branch: &str) -> Result<()> {
        GhCliGateway.delete_branch(repo, branch)
    }
}

// ── Public utility functions ─────────────────────────────────────────────────

/// List open issues with the given label, sorted by number ascending.
///
/// If `label` is empty, returns all open issues.
pub fn list_approved_issues(repo: &str, label: &str) -> Result<Vec<u64>> {
    let mut args = vec![
        "issue", "list", "--repo", repo, "--state", "open", "--json", "number", "--limit", "100",
    ];
    // Allocate label args here to keep them alive for the borrow.
    let label_args;
    if !label.is_empty() {
        label_args = ["--label", label];
        args.extend_from_slice(&label_args);
    }

    let output = Command::new("gh")
        .args(&args)
        .output()
        .context("Failed to run gh issue list — is gh installed and authenticated?")?;

    if !output.status.success() {
        return Ok(vec![]);
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::json!([]));

    let mut issues = vec![];
    if let Some(arr) = parsed.as_array() {
        for item in arr {
            if let Some(n) = item["number"].as_u64() {
                issues.push(n);
            }
        }
    }
    issues.sort_unstable();
    Ok(issues)
}

/// Fetch issue title and body.
pub fn get_issue_details(repo: &str, issue_num: u64) -> Result<(String, String)> {
    let n = issue_num.to_string();
    let output = Command::new("gh")
        .args(["issue", "view", &n, "--repo", repo, "--json", "title,body"])
        .output()
        .context("Failed to run gh issue view")?;

    let text = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::json!({}));

    let title = parsed["title"].as_str().unwrap_or("").to_string();
    let body = parsed["body"].as_str().unwrap_or("").to_string();
    Ok((title, body))
}

/// Retrieve the current GitHub token from the `gh` CLI.
pub fn get_gh_token() -> Option<String> {
    Command::new("gh")
        .args(["auth", "token"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Check whether `gh` is authenticated.
pub fn preflight_gh_auth() -> Result<()> {
    let status = Command::new("gh")
        .args(["auth", "status"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => Ok(()),
        _ => anyhow::bail!("Error: gh is not authenticated.\n\n  To fix:\n\n    gh auth login"),
    }
}

/// Find an open or merged PR for a branch.
pub fn find_pr_for_branch(repo: &str, branch: &str) -> Result<Option<PrInfo>> {
    let output = Command::new("gh")
        .args([
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
            "--limit",
            "1",
        ])
        .output()
        .context("Failed to run gh pr list")?;

    if !output.status.success() {
        return Ok(None);
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::json!([]));

    if let Some(first) = parsed.as_array().and_then(|a| a.first()) {
        let number = first["number"].as_u64().unwrap_or(0);
        let url = first["url"].as_str().unwrap_or("").to_string();
        if number > 0 {
            return Ok(Some(PrInfo {
                number,
                url,
                state: PrState::Open,
                branch: branch.to_string(),
            }));
        }
    }
    Ok(None)
}

/// Transition labels on a GitHub issue.
///
/// Removes `remove` (if non-empty) then adds `add` (if non-empty).
/// Ignores errors (e.g. label already absent or issue closed).
pub fn transition_label(
    repo: &str,
    issue_num: u64,
    remove: Option<&str>,
    add: Option<&str>,
) -> Result<()> {
    let n = issue_num.to_string();
    if let Some(label) = remove {
        if !label.is_empty() {
            let _ = Command::new("gh")
                .args(["issue", "edit", &n, "--repo", repo, "--remove-label", label])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
    if let Some(label) = add {
        if !label.is_empty() {
            let _ = Command::new("gh")
                .args(["issue", "edit", &n, "--repo", repo, "--add-label", label])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
    Ok(())
}

/// Find open PRs that need another worker pass.
///
/// A PR needs iteration when it has a `CHANGES_REQUESTED` review or a new
/// comment posted after the most recent commit. Matches the logic in
/// `lib/worker/github.sh::worker_find_prs_needing_iteration`.
pub fn find_prs_needing_iteration(repo: &str) -> Vec<u64> {
    let jq_filter = r#".[] |
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
        .number"#;

    let output = Command::new("gh")
        .args([
            "pr",
            "list",
            "--repo",
            repo,
            "--state",
            "open",
            "--json",
            "number,reviews,commits,comments",
            "--jq",
            jq_filter,
        ])
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let text = String::from_utf8_lossy(&o.stdout);
            let mut prs: Vec<u64> = text
                .lines()
                .filter_map(|l| l.trim().parse::<u64>().ok())
                .collect();
            prs.sort_unstable();
            prs
        }
        _ => vec![],
    }
}

/// Close issues whose worker-created PRs have since been merged.
///
/// Examines issues labeled `in-progress` and removes the label for any
/// whose associated PR (searched by "closes #N" in body) was merged.
/// Mirrors `lib/worker/github.sh::worker_reconcile`.
pub fn reconcile_merged_prs(repo: &str) -> Result<()> {
    // List issues with the in-progress label.
    let output = Command::new("gh")
        .args([
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
            "--limit",
            "100",
        ])
        .output()
        .context("Failed to list in-progress issues")?;

    if !output.status.success() {
        return Ok(());
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::json!([]));

    if let Some(arr) = parsed.as_array() {
        for item in arr {
            let issue_num = match item["number"].as_u64() {
                Some(n) => n,
                None => continue,
            };

            // Check for a merged PR that closes this issue.
            let search = format!("closes #{issue_num}");
            let pr_out = Command::new("gh")
                .args([
                    "pr", "list", "--repo", repo, "--state", "merged", "--search", &search,
                    "--json", "number", "--limit", "1",
                ])
                .output();

            if let Ok(o) = pr_out {
                let pr_text = String::from_utf8_lossy(&o.stdout);
                let pr_parsed: serde_json::Value =
                    serde_json::from_str(&pr_text).unwrap_or(serde_json::json!([]));
                if pr_parsed.as_array().is_some_and(|a| !a.is_empty()) {
                    // PR merged — remove the in-progress label.
                    let _ = transition_label(repo, issue_num, Some("in-progress"), None);
                    println!("[reconcile] #{issue_num}: removed in-progress (PR merged)");
                }
            }
        }
    }

    Ok(())
}

/// Auto-merge clean sipag PRs.
///
/// Merges open PRs from `sipag/issue-` branches that are mergeable, not
/// draft, and have no changes-requested review. Mirrors
/// `lib/worker/merge.sh::worker_auto_merge`.
pub fn auto_merge_prs(repo: &str) -> Result<()> {
    let jq_filter = r#".[] | select(
        (.headRefName | startswith("sipag/issue-")) and
        .mergeable == "MERGEABLE" and
        .mergeStateStatus == "CLEAN" and
        .isDraft == false and
        .reviewDecision != "CHANGES_REQUESTED"
    ) | .number"#;

    let output = Command::new("gh")
        .args([
            "pr",
            "list",
            "--repo",
            repo,
            "--state",
            "open",
            "--json",
            "number,headRefName,mergeable,mergeStateStatus,isDraft,reviewDecision",
            "--jq",
            jq_filter,
        ])
        .output()
        .context("Failed to list PRs for auto-merge")?;

    if !output.status.success() {
        return Ok(());
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let candidates: Vec<u64> = text
        .lines()
        .filter_map(|l| l.trim().parse::<u64>().ok())
        .collect();

    if candidates.is_empty() {
        return Ok(());
    }

    println!("[auto-merge] {} candidate(s)", candidates.len());

    for pr_num in candidates {
        // Get title for the squash commit message.
        let title_out = Command::new("gh")
            .args([
                "pr",
                "view",
                &pr_num.to_string(),
                "--repo",
                repo,
                "--json",
                "title",
                "--jq",
                ".title",
            ])
            .output();

        let title = title_out
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();

        println!("[auto-merge] Merging PR #{pr_num}: {title}");

        let merge_status = Command::new("gh")
            .args([
                "pr",
                "merge",
                &pr_num.to_string(),
                "--repo",
                repo,
                "--squash",
                "--delete-branch",
                "--subject",
                &title,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        match merge_status {
            Ok(s) if s.success() => println!("[auto-merge] Merged PR #{pr_num}"),
            _ => println!("[auto-merge] Failed to merge PR #{pr_num} (may need manual review)"),
        }
    }

    Ok(())
}

/// Find open PRs with merge conflicts (mergeableState == "CONFLICTING").
///
/// Mirrors `lib/worker/github.sh::worker_find_conflicted_prs`.
pub fn find_conflicted_prs(repo: &str) -> Vec<u64> {
    let jq_filter = r#".[] | select(
        (.headRefName | startswith("sipag/issue-")) and
        .mergeable == "CONFLICTING" and
        .isDraft == false
    ) | .number"#;

    let output = Command::new("gh")
        .args([
            "pr",
            "list",
            "--repo",
            repo,
            "--state",
            "open",
            "--json",
            "number,headRefName,mergeable,isDraft",
            "--jq",
            jq_filter,
        ])
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let text = String::from_utf8_lossy(&o.stdout);
            let mut prs: Vec<u64> = text
                .lines()
                .filter_map(|l| l.trim().parse::<u64>().ok())
                .collect();
            prs.sort_unstable();
            prs
        }
        _ => vec![],
    }
}

/// Get total open issue count (for idle-cycle status display).
pub fn count_open_issues(repo: &str) -> Option<usize> {
    let output = Command::new("gh")
        .args([
            "issue", "list", "--repo", repo, "--state", "open", "--limit", "500", "--json",
            "number", "--jq", "length",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.trim().parse::<usize>().ok()
}

/// Get total open PR count (for idle-cycle status display).
pub fn count_open_prs(repo: &str) -> Option<usize> {
    let output = Command::new("gh")
        .args([
            "pr", "list", "--repo", repo, "--state", "open", "--json", "number", "--jq", "length",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.trim().parse::<usize>().ok()
}
