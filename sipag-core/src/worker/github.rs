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

/// Count open PRs created by sipag (branches matching `sipag/*`).
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
