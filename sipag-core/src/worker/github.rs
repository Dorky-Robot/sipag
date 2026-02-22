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

/// Transition labels on a GitHub issue — idempotent and lifecycle-aware.
///
/// Removes `remove` (if present on the issue) then adds `add` (if not already
/// present and not a lifecycle regression).
///
/// Lifecycle ordering: `approved` < `in-progress` < `needs-review`.
/// Adding a label at a lower lifecycle level than one already present on the
/// issue (after accounting for the remove) is a no-op, preventing races where
/// a stale worker adds `in-progress` to an issue already at `needs-review`.
pub fn transition_label(
    repo: &str,
    issue_num: u64,
    remove: Option<&str>,
    add: Option<&str>,
) -> Result<()> {
    let n = issue_num.to_string();

    // Fetch current labels once for idempotency checks.
    let current_labels = get_current_labels(repo, issue_num);

    if let Some(label) = remove {
        if !label.is_empty() && current_labels.iter().any(|l| l == label) {
            let _ = Command::new("gh")
                .args(["issue", "edit", &n, "--repo", repo, "--remove-label", label])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }

    if let Some(label) = add {
        if !label.is_empty() && !current_labels.iter().any(|l| l == label) {
            // Compute effective labels after the remove, then check lifecycle ordering.
            let effective: Vec<&str> = current_labels
                .iter()
                .filter(|l| remove.map_or(true, |r| l.as_str() != r))
                .map(|l| l.as_str())
                .collect();

            if !is_lifecycle_regression(label, &effective) {
                let _ = Command::new("gh")
                    .args(["issue", "edit", &n, "--repo", repo, "--add-label", label])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
    }

    Ok(())
}

/// Fetch the current label names for a GitHub issue.
///
/// Returns an empty vec on failure (network error, issue not found, etc.).
fn get_current_labels(repo: &str, issue_num: u64) -> Vec<String> {
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
                .map(|labels| {
                    labels
                        .iter()
                        .filter_map(|l| l["name"].as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default()
        }
        _ => vec![],
    }
}

/// Return true if adding `label` would be a lifecycle regression given the
/// `effective_labels` already on the issue.
///
/// Lifecycle order: `approved` (0) < `in-progress` (1) < `needs-review` (2).
/// Adding a label at level N when a label at level > N is already present is
/// a regression and should be skipped.
fn is_lifecycle_regression(label: &str, effective_labels: &[&str]) -> bool {
    const LIFECYCLE: &[&str] = &["approved", "in-progress", "needs-review"];

    let add_level = match LIFECYCLE.iter().position(|&l| l == label) {
        Some(pos) => pos,
        None => return false, // Not a lifecycle label — never a regression.
    };

    let current_max = LIFECYCLE
        .iter()
        .enumerate()
        .filter(|(_, &l)| effective_labels.contains(&l))
        .map(|(i, _)| i)
        .max();

    matches!(current_max, Some(max) if add_level < max)
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
///
/// Post-filters results by fetching the PR body and confirming an exact match,
/// because `gh pr list --search "closes #34"` can also match `closes #344`.
pub fn find_open_pr_for_issue(repo: &str, issue_num: u64) -> Option<PrInfo> {
    let search = format!("closes #{issue_num}");
    let output = Command::new("gh")
        .args([
            "pr",
            "list",
            "--repo",
            repo,
            "--state",
            "open",
            "--search",
            &search,
            "--json",
            "number,url,body",
            "--limit",
            "5",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::json!([]));

    // Post-filter: confirm the PR body contains an exact "closes #N" (not #N0, #N00, etc.).
    let pattern = format!("#{issue_num}");
    if let Some(arr) = parsed.as_array() {
        for item in arr {
            let number = item["number"].as_u64().unwrap_or(0);
            let url = item["url"].as_str().unwrap_or("").to_string();
            let body = item["body"].as_str().unwrap_or("");
            if number == 0 {
                continue;
            }
            // Check that the body contains "closes #N" where N is followed by a
            // non-digit (word boundary) or end of string.
            let lower_body = body.to_lowercase();
            for keyword in &["closes ", "fixes ", "resolves "] {
                let mut search_from = 0;
                while let Some(pos) = lower_body[search_from..].find(keyword) {
                    let abs_pos = search_from + pos + keyword.len();
                    let rest = &body[abs_pos..];
                    if rest.starts_with(&pattern) {
                        let after = &rest[pattern.len()..];
                        // Ensure the next char is not a digit (exact match).
                        if after.is_empty() || !after.starts_with(|c: char| c.is_ascii_digit()) {
                            return Some(PrInfo { number, url });
                        }
                    }
                    search_from = abs_pos;
                }
            }
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

/// Reconcile in-progress issues that have no running container.
///
/// When sipag crashes or a container fails before the cleanup handler runs,
/// issues can get stuck with the `in-progress` label and no active worker.
/// This function checks each in-progress issue against running Docker containers
/// (via worker state files) and reverts orphaned issues back to `work_label`.
pub fn reconcile_stale_in_progress(
    repo: &str,
    work_label: &str,
    is_container_running: impl Fn(&str) -> bool,
    load_worker_state: impl Fn(&str, u64) -> Option<(String, String)>, // -> (container_name, status)
) -> Result<()> {
    let in_progress = list_labeled_issues(repo, "in-progress").unwrap_or_default();
    if in_progress.is_empty() {
        return Ok(());
    }

    for issue_num in &in_progress {
        // Check if we have a state file for this issue.
        if let Some((container_name, status)) = load_worker_state(repo, *issue_num) {
            if status == "running" {
                // State says running — check if Docker container is actually alive.
                if is_container_running(&container_name) {
                    continue; // Container is alive, in-progress is correct.
                }
                // Container is dead but state says running — stale.
                println!(
                    "[reconcile] #{issue_num}: container {container_name} not running, reverting to {work_label}"
                );
            } else if status == "done" || status == "failed" {
                // State is terminal but label is still in-progress — stale.
                println!(
                    "[reconcile] #{issue_num}: worker state is {status}, reverting to {work_label}"
                );
            } else {
                continue; // enqueued or unknown — leave it alone.
            }
        } else {
            // No state file at all — sipag crashed before writing one.
            println!("[reconcile] #{issue_num}: no worker state found, reverting to {work_label}");
        }

        let _ = transition_label(repo, *issue_num, Some("in-progress"), Some(work_label));
    }

    Ok(())
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── is_lifecycle_regression ───────────────────────────────────────────────

    #[test]
    fn regression_in_progress_when_needs_review_present() {
        // The bug scenario: new worker tries to add in-progress, but issue
        // already has needs-review from a prior completed worker.
        assert!(is_lifecycle_regression(
            "in-progress",
            &["needs-review"]
        ));
    }

    #[test]
    fn regression_approved_when_in_progress_present() {
        assert!(is_lifecycle_regression("approved", &["in-progress"]));
    }

    #[test]
    fn regression_approved_when_needs_review_present() {
        assert!(is_lifecycle_regression("approved", &["needs-review"]));
    }

    #[test]
    fn no_regression_needs_review_when_in_progress_present() {
        // Advancing forward is never a regression.
        assert!(!is_lifecycle_regression(
            "needs-review",
            &["in-progress"]
        ));
    }

    #[test]
    fn no_regression_in_progress_when_only_approved_present() {
        assert!(!is_lifecycle_regression("in-progress", &["approved"]));
    }

    #[test]
    fn no_regression_when_no_lifecycle_labels_present() {
        // Empty effective labels: no regression possible.
        assert!(!is_lifecycle_regression("in-progress", &[]));
        assert!(!is_lifecycle_regression("approved", &[]));
        assert!(!is_lifecycle_regression("needs-review", &[]));
    }

    #[test]
    fn no_regression_for_non_lifecycle_label() {
        // Custom labels are never considered lifecycle labels.
        assert!(!is_lifecycle_regression("custom-label", &["needs-review"]));
        assert!(!is_lifecycle_regression("triaged", &["needs-review"]));
    }

    #[test]
    fn no_regression_when_same_level_already_present() {
        // The idempotency check (already present) is handled before lifecycle check,
        // but if it weren't, same-level should not be a regression.
        assert!(!is_lifecycle_regression(
            "in-progress",
            &["in-progress"]
        ));
    }

    #[test]
    fn regression_considers_effective_labels_after_remove() {
        // After removing needs-review, adding approved is not a regression.
        // (Simulates reconcile_closed_prs: remove needs-review, add work_label)
        // effective_labels = [] (needs-review removed)
        assert!(!is_lifecycle_regression("approved", &[]));
    }
}
