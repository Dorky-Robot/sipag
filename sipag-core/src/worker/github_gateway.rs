//! GhCliGateway — implements GitHubGateway by shelling out to the `gh` CLI.
//!
//! All `gh` invocations use structured JSON output (`--json` flags) and are
//! parsed with serde_json into typed Rust structs. No jq queries execute in
//! Rust — the filtering logic lives in decision.rs as pure functions.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::process::Command;

use super::decision::needs_iteration;
use super::ports::{
    Comment, GitHubGateway, IssueInfo, IssueState, PrInfo, PrState, Review, ReviewState,
    TimelineEvent,
};

/// GitHub gateway that shells out to the `gh` CLI.
pub struct GhCliGateway;

impl GhCliGateway {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GhCliGateway {
    fn default() -> Self {
        Self::new()
    }
}

// ── Internal helpers ─────────────────────────────────────────────────────────

/// Run a `gh` command and return stdout. Propagates errors on non-zero exit.
fn run_gh(args: &[&str]) -> Result<String> {
    let output = Command::new("gh")
        .args(args)
        .output()
        .context("failed to spawn gh command")?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(anyhow::anyhow!(
            "gh {} failed: {}",
            args.join(" "),
            stderr.trim()
        ))
    }
}

/// Run a `gh` command that is allowed to fail (mirrors `|| true` in bash).
/// Returns stdout on success, empty string on failure.
fn run_gh_soft(args: &[&str]) -> String {
    Command::new("gh")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// Parse an RFC3339 date string into DateTime<Utc>.
/// Returns Unix epoch on parse failure (treats unknown dates as very old).
fn parse_date(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| {
            DateTime::parse_from_rfc3339("1970-01-01T00:00:00Z")
                .expect("epoch is valid RFC3339")
                .with_timezone(&Utc)
        })
}

/// Map a GitHub API state string + mergedAt to a PrState.
fn parse_pr_state(state: &str, merged_at: Option<&str>) -> PrState {
    let has_merged_at = merged_at.is_some_and(|s| !s.is_empty());
    match state {
        "OPEN" => PrState::Open,
        "MERGED" => PrState::Merged,
        "CLOSED" if has_merged_at => PrState::Merged,
        _ if has_merged_at => PrState::Merged,
        _ => PrState::Closed,
    }
}

/// Check if a PR body references the given issue number with a closing keyword.
///
/// Matches "closes/fixes/resolves #N" (case-insensitive) with a word boundary
/// after the issue number, mirroring the jq regex in github.sh.
fn body_closes_issue(body: &str, issue_num: u64) -> bool {
    let body_lower = body.to_lowercase();
    let issue_tag = format!("#{}", issue_num);

    for keyword in &["closes", "fixes", "resolves"] {
        let mut start = 0;
        let kw = *keyword;
        loop {
            let Some(rel_pos) = body_lower[start..].find(kw) else {
                break;
            };
            let abs_pos = start + rel_pos;
            // After keyword: skip spaces, then expect #N
            let after_kw = body_lower[abs_pos + kw.len()..].trim_start_matches(' ');
            if after_kw.starts_with(issue_tag.as_str()) {
                let rest = &after_kw[issue_tag.len()..];
                // Word boundary: must not be followed by alphanumeric
                if rest.is_empty() || !rest.chars().next().unwrap().is_alphanumeric() {
                    return true;
                }
            }
            start = abs_pos + kw.len();
        }
    }
    false
}

// ── GitHubGateway implementation ──────────────────────────────────────────────

impl GitHubGateway for GhCliGateway {
    fn find_pr_for_branch(&self, repo: &str, branch: &str) -> Result<Option<PrInfo>> {
        let output = match run_gh(&[
            "pr",
            "list",
            "--repo",
            repo,
            "--head",
            branch,
            "--state",
            "all",
            "--json",
            "number,url,state,mergedAt,headRefName",
        ]) {
            Ok(out) => out,
            Err(e) => {
                eprintln!("[worker] WARNING: gh pr list failed for {repo} branch {branch}: {e}");
                return Ok(None);
            }
        };

        let prs: serde_json::Value = serde_json::from_str(&output)?;
        if let Some(arr) = prs.as_array() {
            for pr in arr {
                let state_str = pr["state"].as_str().unwrap_or("");
                let merged_at = pr["mergedAt"].as_str();
                let state = parse_pr_state(state_str, merged_at);
                if state == PrState::Closed {
                    continue;
                }
                return Ok(Some(PrInfo {
                    number: pr["number"].as_u64().unwrap_or(0),
                    url: pr["url"].as_str().unwrap_or("").to_string(),
                    state,
                    branch: pr["headRefName"].as_str().unwrap_or(branch).to_string(),
                }));
            }
        }
        Ok(None)
    }

    fn find_pr_for_issue(&self, repo: &str, issue_num: u64) -> Result<Option<PrInfo>> {
        let search = format!("closes #{}", issue_num);
        let output = match run_gh(&[
            "pr",
            "list",
            "--repo",
            repo,
            "--state",
            "all",
            "--search",
            &search,
            "--json",
            "number,body,state,mergedAt,headRefName,url",
        ]) {
            Ok(out) => out,
            Err(e) => {
                eprintln!("[worker] WARNING: gh pr list failed for {repo} issue #{issue_num}: {e}");
                return Ok(None);
            }
        };

        let prs: serde_json::Value = serde_json::from_str(&output)?;
        if let Some(arr) = prs.as_array() {
            for pr in arr {
                let body = pr["body"].as_str().unwrap_or("");
                if !body_closes_issue(body, issue_num) {
                    continue;
                }
                let state_str = pr["state"].as_str().unwrap_or("");
                let merged_at = pr["mergedAt"].as_str();
                let state = parse_pr_state(state_str, merged_at);
                // Exclude closed-without-merge so abandoned PRs don't block re-dispatch
                if state == PrState::Closed {
                    continue;
                }
                return Ok(Some(PrInfo {
                    number: pr["number"].as_u64().unwrap_or(0),
                    url: pr["url"].as_str().unwrap_or("").to_string(),
                    state,
                    branch: pr["headRefName"].as_str().unwrap_or("").to_string(),
                }));
            }
        }
        Ok(None)
    }

    fn find_open_pr_for_issue(&self, repo: &str, issue_num: u64) -> Result<Option<PrInfo>> {
        let search = format!("closes #{}", issue_num);
        let output = match run_gh(&[
            "pr",
            "list",
            "--repo",
            repo,
            "--state",
            "open",
            "--search",
            &search,
            "--json",
            "number,body,url,headRefName",
        ]) {
            Ok(out) => out,
            Err(e) => {
                eprintln!("[worker] WARNING: gh pr list failed for {repo} issue #{issue_num}: {e}");
                return Ok(None);
            }
        };

        let prs: serde_json::Value = serde_json::from_str(&output)?;
        if let Some(arr) = prs.as_array() {
            for pr in arr {
                let body = pr["body"].as_str().unwrap_or("");
                if body_closes_issue(body, issue_num) {
                    return Ok(Some(PrInfo {
                        number: pr["number"].as_u64().unwrap_or(0),
                        url: pr["url"].as_str().unwrap_or("").to_string(),
                        state: PrState::Open,
                        branch: pr["headRefName"].as_str().unwrap_or("").to_string(),
                    }));
                }
            }
        }
        Ok(None)
    }

    fn find_prs_needing_iteration(&self, repo: &str) -> Result<Vec<u64>> {
        let epoch = parse_date("1970-01-01T00:00:00Z");

        let output = match run_gh(&[
            "pr",
            "list",
            "--repo",
            repo,
            "--state",
            "open",
            "--json",
            "number,reviews,commits,comments",
        ]) {
            Ok(out) => out,
            Err(_) => return Ok(vec![]),
        };

        let prs: serde_json::Value = serde_json::from_str(&output)?;
        let mut needing = Vec::new();

        if let Some(arr) = prs.as_array() {
            for pr in arr {
                let pr_num = pr["number"].as_u64().unwrap_or(0);

                // Last commit date (epoch if no commits)
                let last_commit_date = pr["commits"]
                    .as_array()
                    .and_then(|commits| commits.last())
                    .and_then(|c| c["committedDate"].as_str())
                    .map(parse_date)
                    .unwrap_or(epoch);

                // Parse reviews
                let reviews: Vec<Review> = pr["reviews"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|r| {
                                let state_str = r["state"].as_str()?;
                                let submitted_at = r["submittedAt"].as_str().map(parse_date)?;
                                let state = match state_str {
                                    "CHANGES_REQUESTED" => ReviewState::ChangesRequested,
                                    "APPROVED" => ReviewState::Approved,
                                    "COMMENTED" => ReviewState::Commented,
                                    "DISMISSED" => ReviewState::Dismissed,
                                    _ => ReviewState::Other,
                                };
                                Some(Review {
                                    state,
                                    submitted_at,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                // Parse comments
                let comments: Vec<Comment> = pr["comments"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|c| {
                                let created_at = c["createdAt"].as_str().map(parse_date)?;
                                Some(Comment { created_at })
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                if needs_iteration(&reviews, &comments, last_commit_date) {
                    needing.push(pr_num);
                }
            }
        }

        needing.sort();
        Ok(needing)
    }

    fn find_conflicted_prs(&self, repo: &str) -> Result<Vec<PrInfo>> {
        let output = match run_gh(&[
            "pr",
            "list",
            "--repo",
            repo,
            "--state",
            "open",
            "--json",
            "number,headRefName,mergeable,url",
        ]) {
            Ok(out) => out,
            Err(_) => return Ok(vec![]),
        };

        let prs: serde_json::Value = serde_json::from_str(&output)?;
        let mut conflicted = Vec::new();

        if let Some(arr) = prs.as_array() {
            for pr in arr {
                let branch = pr["headRefName"].as_str().unwrap_or("");
                let mergeable = pr["mergeable"].as_str().unwrap_or("");
                // Only sipag-managed branches with confirmed conflicts (not UNKNOWN)
                if branch.starts_with("sipag/issue-") && mergeable == "CONFLICTING" {
                    conflicted.push(PrInfo {
                        number: pr["number"].as_u64().unwrap_or(0),
                        url: pr["url"].as_str().unwrap_or("").to_string(),
                        state: PrState::Open,
                        branch: branch.to_string(),
                    });
                }
            }
        }

        conflicted.sort_by_key(|p| p.number);
        Ok(conflicted)
    }

    fn issue_is_open(&self, repo: &str, issue_num: u64) -> Result<bool> {
        let num_str = issue_num.to_string();
        let output = match run_gh(&["issue", "view", &num_str, "--repo", repo, "--json", "state"]) {
            Ok(out) => out,
            Err(_) => return Ok(false),
        };

        let v: serde_json::Value = serde_json::from_str(&output)?;
        Ok(v["state"].as_str() == Some("OPEN"))
    }

    fn get_issue_info(&self, repo: &str, issue_num: u64) -> Result<Option<IssueInfo>> {
        let num_str = issue_num.to_string();
        let output = match run_gh(&[
            "issue",
            "view",
            &num_str,
            "--repo",
            repo,
            "--json",
            "number,title,body,state",
        ]) {
            Ok(out) => out,
            Err(_) => return Ok(None),
        };

        let v: serde_json::Value = serde_json::from_str(&output)?;
        let state = match v["state"].as_str() {
            Some("OPEN") => IssueState::Open,
            _ => IssueState::Closed,
        };

        Ok(Some(IssueInfo {
            number: v["number"].as_u64().unwrap_or(issue_num),
            title: v["title"].as_str().unwrap_or("").to_string(),
            body: v["body"].as_str().unwrap_or("").to_string(),
            state,
        }))
    }

    fn list_issues_with_label(&self, repo: &str, label: &str) -> Result<Vec<u64>> {
        let output = match run_gh(&[
            "issue", "list", "--repo", repo, "--state", "open", "--label", label, "--json",
            "number",
        ]) {
            Ok(out) => out,
            Err(_) => return Ok(vec![]),
        };

        let v: serde_json::Value = serde_json::from_str(&output)?;
        let mut numbers = Vec::new();

        if let Some(arr) = v.as_array() {
            for issue in arr {
                if let Some(n) = issue["number"].as_u64() {
                    numbers.push(n);
                }
            }
        }

        numbers.sort();
        Ok(numbers)
    }

    fn get_issue_timeline(&self, repo: &str, issue_num: u64) -> Result<Vec<TimelineEvent>> {
        let endpoint = format!("repos/{}/issues/{}/timeline", repo, issue_num);
        let output = match run_gh(&["api", &endpoint]) {
            Ok(out) => out,
            Err(e) => {
                eprintln!("[worker] WARNING: gh api timeline failed for {repo}#{issue_num}: {e}");
                return Ok(vec![]);
            }
        };

        let events: serde_json::Value = serde_json::from_str(&output)?;
        let mut timeline = Vec::new();

        if let Some(arr) = events.as_array() {
            for event in arr {
                let event_type = event["event"].as_str().unwrap_or("");
                if event_type == "cross-referenced" {
                    // Check if the referencing PR was merged
                    let merged_at = event["source"]["issue"]["pull_request"]["merged_at"].as_str();
                    let merged = merged_at.is_some_and(|s| !s.is_empty());
                    if let Some(pr_num) = event["source"]["issue"]["number"].as_u64() {
                        timeline.push(TimelineEvent::CrossReferenced { pr_num, merged });
                        continue;
                    }
                }
                timeline.push(TimelineEvent::Other);
            }
        }

        Ok(timeline)
    }

    fn close_issue(&self, repo: &str, issue_num: u64, comment: &str) -> Result<()> {
        let num_str = issue_num.to_string();
        run_gh_soft(&[
            "issue",
            "close",
            &num_str,
            "--repo",
            repo,
            "--comment",
            comment,
        ]);
        Ok(())
    }

    fn transition_label(
        &self,
        repo: &str,
        issue_num: u64,
        remove: Option<&str>,
        add: Option<&str>,
    ) -> Result<()> {
        let num_str = issue_num.to_string();
        // Use soft (non-failing) gh calls, mirroring `|| true` in github.sh.
        if let Some(label) = remove {
            run_gh_soft(&[
                "issue",
                "edit",
                &num_str,
                "--repo",
                repo,
                "--remove-label",
                label,
            ]);
        }
        if let Some(label) = add {
            run_gh_soft(&[
                "issue",
                "edit",
                &num_str,
                "--repo",
                repo,
                "--add-label",
                label,
            ]);
        }
        Ok(())
    }

    fn get_pr_info(&self, repo: &str, pr_num: u64) -> Result<Option<PrInfo>> {
        let num_str = pr_num.to_string();
        let output = match run_gh(&[
            "pr",
            "view",
            &num_str,
            "--repo",
            repo,
            "--json",
            "number,url,state,mergedAt,headRefName",
        ]) {
            Ok(out) => out,
            Err(_) => return Ok(None),
        };

        let v: serde_json::Value = serde_json::from_str(&output)?;
        let state_str = v["state"].as_str().unwrap_or("");
        let merged_at = v["mergedAt"].as_str();
        let state = parse_pr_state(state_str, merged_at);

        Ok(Some(PrInfo {
            number: v["number"].as_u64().unwrap_or(pr_num),
            url: v["url"].as_str().unwrap_or("").to_string(),
            state,
            branch: v["headRefName"].as_str().unwrap_or("").to_string(),
        }))
    }

    fn get_open_prs_for_branch(&self, repo: &str, branch: &str) -> Result<Vec<PrInfo>> {
        let output = match run_gh(&[
            "pr",
            "list",
            "--repo",
            repo,
            "--head",
            branch,
            "--state",
            "open",
            "--json",
            "number,url,headRefName",
        ]) {
            Ok(out) => out,
            Err(_) => return Ok(vec![]),
        };

        parse_pr_list_json(&output, PrState::Open, branch)
    }

    fn get_merged_prs_for_branch(&self, repo: &str, branch: &str) -> Result<Vec<PrInfo>> {
        let output = match run_gh(&[
            "pr",
            "list",
            "--repo",
            repo,
            "--head",
            branch,
            "--state",
            "merged",
            "--json",
            "number,url,headRefName",
        ]) {
            Ok(out) => out,
            Err(_) => return Ok(vec![]),
        };

        parse_pr_list_json(&output, PrState::Merged, branch)
    }

    fn list_branches_with_prefix(&self, repo: &str, prefix: &str) -> Result<Vec<String>> {
        let endpoint = format!("repos/{}/branches?per_page=100", repo);
        let output = match run_gh(&["api", &endpoint]) {
            Ok(out) => out,
            Err(_) => return Ok(vec![]),
        };

        let v: serde_json::Value = serde_json::from_str(&output)?;
        let mut branches = Vec::new();

        if let Some(arr) = v.as_array() {
            for branch in arr {
                if let Some(name) = branch["name"].as_str() {
                    if name.starts_with(prefix) {
                        branches.push(name.to_string());
                    }
                }
            }
        }

        Ok(branches)
    }

    fn branch_ahead_by(&self, repo: &str, base: &str, head: &str) -> Result<u64> {
        let endpoint = format!("repos/{}/compare/{}...{}", repo, base, head);
        let output = match run_gh(&["api", &endpoint]) {
            Ok(out) => out,
            Err(_) => return Ok(0),
        };

        let v: serde_json::Value = serde_json::from_str(&output)?;
        Ok(v["ahead_by"].as_u64().unwrap_or(0))
    }

    fn create_pr(&self, repo: &str, branch: &str, title: &str, body: &str) -> Result<()> {
        run_gh_soft(&[
            "pr", "create", "--repo", repo, "--title", title, "--body", body, "--head", branch,
        ]);
        Ok(())
    }

    fn delete_branch(&self, repo: &str, branch: &str) -> Result<()> {
        let endpoint = format!("repos/{}/git/refs/heads/{}", repo, branch);
        run_gh_soft(&["api", "-X", "DELETE", &endpoint]);
        Ok(())
    }
}

/// Parse a `gh pr list --json number,url,headRefName` response into Vec<PrInfo>.
fn parse_pr_list_json(
    output: &str,
    default_state: PrState,
    default_branch: &str,
) -> Result<Vec<PrInfo>> {
    let v: serde_json::Value = serde_json::from_str(output)?;
    let mut prs = Vec::new();

    if let Some(arr) = v.as_array() {
        for pr in arr {
            prs.push(PrInfo {
                number: pr["number"].as_u64().unwrap_or(0),
                url: pr["url"].as_str().unwrap_or("").to_string(),
                state: default_state.clone(),
                branch: pr["headRefName"]
                    .as_str()
                    .unwrap_or(default_branch)
                    .to_string(),
            });
        }
    }

    Ok(prs)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── body_closes_issue ─────────────────────────────────────────────────────

    #[test]
    fn closes_lowercase_matches() {
        assert!(body_closes_issue("closes #42", 42));
    }

    #[test]
    fn closes_uppercase_matches() {
        assert!(body_closes_issue("Closes #42", 42));
    }

    #[test]
    fn fixes_matches() {
        assert!(body_closes_issue("fixes #42", 42));
    }

    #[test]
    fn resolves_matches() {
        assert!(body_closes_issue("resolves #42", 42));
    }

    #[test]
    fn different_issue_num_no_match() {
        assert!(!body_closes_issue("closes #43", 42));
    }

    #[test]
    fn partial_number_no_match() {
        // closes #4 should NOT match issue #42 (word boundary after 4)
        assert!(!body_closes_issue("closes #42", 4));
    }

    #[test]
    fn no_keyword_no_match() {
        assert!(!body_closes_issue("referenced #42", 42));
    }

    #[test]
    fn in_pr_body_multiline() {
        let body = "This PR implements the feature.\n\nCloses #42\n\nSome more text.";
        assert!(body_closes_issue(body, 42));
    }

    #[test]
    fn followed_by_period_matches() {
        // Period is not alphanumeric — word boundary holds
        assert!(body_closes_issue("closes #42.", 42));
    }

    #[test]
    fn followed_by_digit_no_match() {
        // "closes #421" should NOT match #42
        assert!(!body_closes_issue("closes #421", 42));
    }

    // ── parse_pr_state ─────────────────────────────────────────────────────────

    #[test]
    fn open_state_parsed() {
        assert_eq!(parse_pr_state("OPEN", None), PrState::Open);
    }

    #[test]
    fn merged_state_parsed() {
        assert_eq!(parse_pr_state("MERGED", None), PrState::Merged);
    }

    #[test]
    fn closed_with_merged_at_is_merged() {
        assert_eq!(
            parse_pr_state("CLOSED", Some("2024-01-15T10:00:00Z")),
            PrState::Merged
        );
    }

    #[test]
    fn closed_without_merged_at_is_closed() {
        assert_eq!(parse_pr_state("CLOSED", None), PrState::Closed);
    }

    // ── parse_date ─────────────────────────────────────────────────────────────

    #[test]
    fn valid_rfc3339_parses() {
        use chrono::Datelike;
        let dt = parse_date("2024-01-15T10:30:00Z");
        assert_eq!(dt.year(), 2024);
    }

    #[test]
    fn invalid_date_returns_epoch() {
        use chrono::Datelike;
        let dt = parse_date("not-a-date");
        assert_eq!(dt.year(), 1970);
    }
}
