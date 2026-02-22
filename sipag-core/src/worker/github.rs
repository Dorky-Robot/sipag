//! GitHub operations via the `gh` CLI.
//!
//! Mirrors `lib/worker/github.sh` and implements `GitHubGateway` for
//! production use with the `gh` CLI tool.

use anyhow::{Context, Result};
use std::process::{Command, Stdio};

use super::ports::{GitHubGateway, PrInfo};

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
        find_pr_for_branch(repo, branch)
    }

    fn transition_label(
        &self,
        repo: &str,
        issue_num: u64,
        remove: Option<&str>,
        add: Option<&str>,
    ) -> Result<()> {
        transition_label(repo, issue_num, remove, add)
    }
}

// ── Public utility functions ─────────────────────────────────────────────────

/// List open issues with the given label, sorted by number ascending.
///
/// If `label` is empty, returns all open issues.
pub fn list_labeled_issues(repo: &str, label: &str) -> Result<Vec<u64>> {
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
            return Ok(Some(PrInfo { number, url }));
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

/// Revert labels on issues whose worker PRs were closed without merging.
///
/// Examines issues labeled `needs-review` and checks whether their associated
/// PR (searched by "closes #N" in body) is closed (not merged) with no open
/// replacement. If so, reverts the label to `work_label` so the issue can be
/// re-dispatched.
pub fn reconcile_closed_prs(repo: &str, work_label: &str) -> Result<()> {
    let output = Command::new("gh")
        .args([
            "issue",
            "list",
            "--repo",
            repo,
            "--state",
            "open",
            "--label",
            "needs-review",
            "--json",
            "number",
            "--limit",
            "100",
        ])
        .output()
        .context("Failed to list needs-review issues")?;

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

            // Check for an open PR that closes this issue — if one exists, leave it alone.
            let search = format!("closes #{issue_num}");
            let open_out = Command::new("gh")
                .args([
                    "pr", "list", "--repo", repo, "--state", "open", "--search", &search, "--json",
                    "number", "--limit", "1",
                ])
                .output();

            if let Ok(o) = &open_out {
                let pr_text = String::from_utf8_lossy(&o.stdout);
                let pr_parsed: serde_json::Value =
                    serde_json::from_str(&pr_text).unwrap_or(serde_json::json!([]));
                if pr_parsed.as_array().is_some_and(|a| !a.is_empty()) {
                    continue; // Open PR exists — needs-review is correct.
                }
            }

            // Check for a closed (not merged) PR.
            let closed_out = Command::new("gh")
                .args([
                    "pr",
                    "list",
                    "--repo",
                    repo,
                    "--state",
                    "closed",
                    "--search",
                    &search,
                    "--json",
                    "number,mergedAt",
                    "--limit",
                    "1",
                ])
                .output();

            if let Ok(o) = closed_out {
                let pr_text = String::from_utf8_lossy(&o.stdout);
                let pr_parsed: serde_json::Value =
                    serde_json::from_str(&pr_text).unwrap_or(serde_json::json!([]));
                if let Some(first) = pr_parsed.as_array().and_then(|a| a.first()) {
                    // Only revert if the PR was closed WITHOUT merging.
                    let was_merged = first["mergedAt"].as_str().is_some_and(|s| !s.is_empty());
                    if !was_merged {
                        let _ = transition_label(
                            repo,
                            issue_num,
                            Some("needs-review"),
                            Some(work_label),
                        );
                        println!("[reconcile] #{issue_num}: reverted to {work_label} (PR closed without merge)");
                    }
                }
            }
        }
    }

    Ok(())
}

/// Find open PRs with merge conflicts (mergeableState == "CONFLICTING").
///
/// Mirrors `lib/worker/github.sh::worker_find_conflicted_prs`.
pub fn find_conflicted_prs(repo: &str) -> Vec<u64> {
    let jq_filter = r#".[] | select(
        ((.headRefName | startswith("sipag/issue-")) or (.headRefName | startswith("sipag/group-"))) and
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

/// Find the first open PR whose body references "Closes #N" for the given issue.
///
/// This catches both single-issue workers (`sipag/issue-N-*`) and grouped
/// workers (`sipag/group-*`) that have already addressed an issue, so that
/// the dispatch loop does not create duplicate PRs.
pub fn find_open_pr_for_issue(repo: &str, issue_num: u64) -> Option<PrInfo> {
    let search = format!("closes #{issue_num}");
    let output = Command::new("gh")
        .args([
            "pr", "list", "--repo", repo, "--state", "open", "--search", &search, "--json",
            "number,url", "--limit", "1",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::json!([]));

    if let Some(first) = parsed.as_array().and_then(|a| a.first()) {
        let number = first["number"].as_u64().unwrap_or(0);
        let url = first["url"].as_str().unwrap_or("").to_string();
        if number > 0 {
            return Some(PrInfo { number, url });
        }
    }
    None
}

/// Check whether an issue currently has the given label.
pub fn issue_has_label(repo: &str, issue_num: u64, label: &str) -> bool {
    let output = Command::new("gh")
        .args([
            "issue",
            "view",
            &issue_num.to_string(),
            "--repo",
            repo,
            "--json",
            "labels",
        ])
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let text = String::from_utf8_lossy(&o.stdout);
            let parsed: serde_json::Value =
                serde_json::from_str(&text).unwrap_or(serde_json::json!({}));
            parsed["labels"]
                .as_array()
                .is_some_and(|labels| labels.iter().any(|l| l["name"].as_str() == Some(label)))
        }
        _ => false,
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

/// Count open PRs created by sipag (branches matching `sipag/*`).
///
/// Used for back-pressure: when the count reaches the configured `max_open_prs`
/// threshold, new issue dispatch is paused until PRs are merged or closed.
pub fn count_open_sipag_prs(repo: &str) -> Option<usize> {
    let output = Command::new("gh")
        .args([
            "pr",
            "list",
            "--repo",
            repo,
            "--state",
            "open",
            "--json",
            "headRefName",
            "--jq",
            r#"[.[] | select(.headRefName | startswith("sipag/"))] | length"#,
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.trim().parse::<usize>().ok()
}
