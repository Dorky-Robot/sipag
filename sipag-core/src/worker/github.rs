//! GitHub operations via the `gh` CLI.

use anyhow::{Context, Result};
use std::process::{Command, Stdio};

/// List open issues with the given label, sorted by number ascending.
pub fn list_labeled_issues(repo: &str, label: &str) -> Result<Vec<u64>> {
    const LIMIT: usize = 100;
    let limit_str = LIMIT.to_string();
    let mut args = vec![
        "issue", "list", "--repo", repo, "--state", "open", "--json", "number", "--limit",
        &limit_str,
    ];
    let label_args;
    if !label.is_empty() {
        label_args = ["--label", label];
        args.extend_from_slice(&label_args);
    }

    let output = Command::new("gh")
        .args(&args)
        .output()
        .context("Failed to run gh issue list")?;

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
        if arr.len() == LIMIT {
            eprintln!("sipag warning: list_labeled_issues returned {LIMIT} issues (limit reached)");
        }
    }
    issues.sort_unstable();
    Ok(issues)
}

/// Count open PRs created by sipag (labeled `sipag`).
pub fn count_open_sipag_prs(repo: &str) -> Option<usize> {
    let output = Command::new("gh")
        .args([
            "pr", "list", "--repo", repo, "--state", "open", "--label", "sipag", "--json",
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

/// Ensure the `sipag` label exists on a repo (idempotent).
pub fn ensure_sipag_label(repo: &str) {
    let status = Command::new("gh")
        .args([
            "label",
            "create",
            "sipag",
            "--repo",
            repo,
            "--color",
            "8B5CF6",
            "--description",
            "PR managed by sipag",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    // Label already existing is fine (gh exits 0 or 1 for "already exists").
    if let Err(e) = status {
        eprintln!("sipag warning: failed to ensure sipag label on {repo}: {e}");
    }
}

/// Add the `sipag` label to a PR.
pub fn label_pr_sipag(repo: &str, pr_num: u64) {
    let n = pr_num.to_string();
    let output = Command::new("gh")
        .args(["pr", "edit", &n, "--repo", repo, "--add-label", "sipag"])
        .stdout(Stdio::null())
        .output();
    match output {
        Ok(o) if !o.status.success() => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            eprintln!("sipag warning: failed to label PR #{pr_num} on {repo}: {stderr}");
        }
        Err(e) => {
            eprintln!("sipag warning: failed to label PR #{pr_num} on {repo}: {e}");
        }
        _ => {}
    }
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
        _ => anyhow::bail!("gh is not authenticated. Run `gh auth login`."),
    }
}

/// Summary of a GitHub issue for board state display.
pub struct IssueSummary {
    pub number: u64,
    pub title: String,
    pub labels: Vec<String>,
}

/// Summary of a GitHub PR for board state display.
pub struct PrSummary {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub labels: Vec<String>,
}

/// Fetch open issues for a repo with titles and labels.
pub fn fetch_open_issues(repo: &str) -> Result<Vec<IssueSummary>> {
    let output = Command::new("gh")
        .args([
            "issue",
            "list",
            "--repo",
            repo,
            "--state",
            "open",
            "--json",
            "number,title,labels",
            "--limit",
            "100",
        ])
        .output()
        .context("Failed to run gh issue list")?;

    if !output.status.success() {
        return Ok(vec![]);
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::json!([]));

    let mut issues = vec![];
    if let Some(arr) = parsed.as_array() {
        for item in arr {
            let number = item["number"].as_u64().unwrap_or(0);
            let title = item["title"].as_str().unwrap_or("").to_string();
            let labels = item["labels"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|l| l["name"].as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            if number > 0 {
                issues.push(IssueSummary {
                    number,
                    title,
                    labels,
                });
            }
        }
    }
    issues.sort_by_key(|i| i.number);
    Ok(issues)
}

/// Fetch open PRs for a repo with titles, state, and labels.
pub fn fetch_open_prs(repo: &str) -> Result<Vec<PrSummary>> {
    let output = Command::new("gh")
        .args([
            "pr",
            "list",
            "--repo",
            repo,
            "--state",
            "open",
            "--json",
            "number,title,state,labels",
            "--limit",
            "100",
        ])
        .output()
        .context("Failed to run gh pr list")?;

    if !output.status.success() {
        return Ok(vec![]);
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::json!([]));

    let mut prs = vec![];
    if let Some(arr) = parsed.as_array() {
        for item in arr {
            let number = item["number"].as_u64().unwrap_or(0);
            let title = item["title"].as_str().unwrap_or("").to_string();
            let state = item["state"].as_str().unwrap_or("OPEN").to_string();
            let labels = item["labels"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|l| l["name"].as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            if number > 0 {
                prs.push(PrSummary {
                    number,
                    title,
                    state,
                    labels,
                });
            }
        }
    }
    prs.sort_by_key(|p| p.number);
    Ok(prs)
}

/// Transition labels on a batch of GitHub issues.
///
/// Removes `remove_label` and adds `add_label` on each issue.
pub fn label_issues(
    repo: &str,
    issue_nums: &[u64],
    remove_label: Option<&str>,
    add_label: Option<&str>,
) -> Result<()> {
    for &num in issue_nums {
        let n = num.to_string();

        if let Some(label) = remove_label {
            let _ = Command::new("gh")
                .args(["issue", "edit", &n, "--repo", repo, "--remove-label", label])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        if let Some(label) = add_label {
            let _ = Command::new("gh")
                .args(["issue", "edit", &n, "--repo", repo, "--add-label", label])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
    Ok(())
}
