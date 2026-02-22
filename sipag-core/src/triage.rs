//! Automated backlog triage: evaluate open issues against VISION.md.
//!
//! Reads VISION.md (and optionally ARCHITECTURE.md) from a GitHub repo,
//! fetches all open issues, asks Claude to evaluate each one, and
//! recommends CLOSE / ADJUST / KEEP / MERGE actions.

use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::io::{self, Write};
use std::process::{Command, Stdio};

// ── Data types ────────────────────────────────────────────────────────────────

/// An open GitHub issue.
#[derive(Debug, Clone)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
}

/// Recommended action for a single issue.
#[derive(Debug, Clone)]
pub enum Action {
    /// Close — conflicts with vision, non-goal, or superseded.
    Close,
    /// Adjust labels/priority.
    Adjust { labels: Vec<String> },
    /// Keep as-is — aligns with vision.
    Keep,
    /// Close as duplicate of `into`.
    Merge { into: u64 },
}

/// Triage recommendation for one issue.
#[derive(Debug)]
pub struct Recommendation {
    pub number: u64,
    pub title: String,
    pub action: Action,
    pub reason: String,
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Run `sipag triage <owner/repo>`.
///
/// - `dry_run` — print report but make no changes.
/// - `apply`   — apply recommendations without prompting.
pub fn run_triage(repo: &str, dry_run: bool, apply: bool) -> Result<()> {
    let (owner, repo_name) = parse_repo(repo)?;

    println!("sipag triage {repo}");
    println!();

    // ── Fetch project context ─────────────────────────────────────────────
    eprint!("Fetching VISION.md... ");
    let vision = fetch_file_raw(&owner, &repo_name, "VISION.md")?.ok_or_else(|| {
        anyhow::anyhow!(
            "VISION.md not found in {repo}.\n\
             Triage requires a VISION.md at the repository root."
        )
    })?;
    eprintln!("ok");

    eprint!("Fetching ARCHITECTURE.md... ");
    let architecture = fetch_file_raw(&owner, &repo_name, "ARCHITECTURE.md")?;
    if architecture.is_none() {
        eprintln!("not found (continuing without it)");
    } else {
        eprintln!("ok");
    }

    // ── Fetch open issues ─────────────────────────────────────────────────
    eprint!("Fetching open issues... ");
    let issues = fetch_open_issues(&owner, &repo_name)?;
    eprintln!("{} found", issues.len());

    if issues.is_empty() {
        println!("No open issues to triage.");
        return Ok(());
    }

    // ── Claude analysis ───────────────────────────────────────────────────
    println!(
        "Reviewing {} open issues against VISION.md...",
        issues.len()
    );
    println!();
    eprint!("Analyzing with Claude... ");

    let prompt = build_triage_prompt(repo, &vision, architecture.as_deref(), &issues);
    let raw = run_claude_capture(&prompt).context("Failed to run Claude analysis")?;
    eprintln!("done");
    println!();

    let recommendations =
        parse_recommendations(&raw, &issues).context("Failed to parse Claude recommendations")?;

    // ── Report ────────────────────────────────────────────────────────────
    display_report(&recommendations);

    let n_close = recommendations
        .iter()
        .filter(|r| matches!(r.action, Action::Close))
        .count();
    let n_merge = recommendations
        .iter()
        .filter(|r| matches!(r.action, Action::Merge { .. }))
        .count();
    let n_adjust = recommendations
        .iter()
        .filter(|r| matches!(r.action, Action::Adjust { .. }))
        .count();
    let n_keep = recommendations
        .iter()
        .filter(|r| matches!(r.action, Action::Keep))
        .count();

    let mut parts: Vec<String> = Vec::new();
    if n_close > 0 {
        parts.push(format!("{n_close} close"));
    }
    if n_merge > 0 {
        parts.push(format!("{n_merge} merge"));
    }
    if n_adjust > 0 {
        parts.push(format!("{n_adjust} adjust"));
    }
    if n_keep > 0 {
        parts.push(format!("{n_keep} keep"));
    }
    println!("\n{}", parts.join(", "));

    if dry_run {
        println!("\n(dry-run: no changes applied)");
        return Ok(());
    }

    // ── Confirmation ──────────────────────────────────────────────────────
    if !apply {
        print!("\nApply? [y/N] ");
        io::stdout().flush().ok();
        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .context("Failed to read confirmation")?;
        if !matches!(input.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
    }

    // ── Apply ─────────────────────────────────────────────────────────────
    println!();
    apply_changes(&owner, &repo_name, &recommendations)?;
    println!("\nDone.");

    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Parse `"owner/repo"` into `(owner, repo_name)`.
pub fn parse_repo(repo: &str) -> Result<(String, String)> {
    match repo.split_once('/') {
        Some((owner, name)) if !owner.is_empty() && !name.is_empty() => {
            Ok((owner.to_string(), name.to_string()))
        }
        _ => bail!("Invalid repo format '{repo}'. Expected 'owner/repo'."),
    }
}

/// Fetch raw file content from GitHub via `gh api`.
///
/// Returns `Ok(None)` when the file does not exist (HTTP 404).
fn fetch_file_raw(owner: &str, repo: &str, path: &str) -> Result<Option<String>> {
    let output = Command::new("gh")
        .args([
            "api",
            &format!("repos/{owner}/{repo}/contents/{path}"),
            "-H",
            "Accept: application/vnd.github.raw",
        ])
        .output()
        .context("Failed to run `gh api`. Is `gh` installed and authenticated?")?;

    if output.status.success() {
        let content = String::from_utf8(output.stdout)
            .with_context(|| format!("Failed to decode {path} as UTF-8"))?;
        Ok(Some(content))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("404") || stderr.to_lowercase().contains("not found") {
            Ok(None)
        } else {
            bail!("Failed to fetch {path}: {stderr}");
        }
    }
}

/// Fetch up to 200 open issues from the repository.
fn fetch_open_issues(owner: &str, repo: &str) -> Result<Vec<Issue>> {
    let output = Command::new("gh")
        .args([
            "issue",
            "list",
            "--repo",
            &format!("{owner}/{repo}"),
            "--state",
            "open",
            "--json",
            "number,title,body,labels",
            "--limit",
            "200",
        ])
        .output()
        .context("Failed to run `gh issue list`")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Failed to fetch issues from {owner}/{repo}: {stderr}");
    }

    let json: Value =
        serde_json::from_slice(&output.stdout).context("Failed to parse issue list as JSON")?;

    let issues = json
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Expected JSON array from `gh issue list`"))?
        .iter()
        .filter_map(|item| {
            let number = item["number"].as_u64()?;
            let title = item["title"].as_str()?.to_string();
            let body = item["body"].as_str().unwrap_or("").to_string();
            let labels = item["labels"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|l| l["name"].as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            Some(Issue {
                number,
                title,
                body,
                labels,
            })
        })
        .collect();

    Ok(issues)
}

/// Build the triage prompt for Claude.
pub fn build_triage_prompt(
    repo: &str,
    vision: &str,
    architecture: Option<&str>,
    issues: &[Issue],
) -> String {
    let mut prompt = format!("You are reviewing the issue backlog for {repo}.\n\n");

    prompt.push_str("=== PROJECT VISION ===\n");
    prompt.push_str(vision.trim());
    prompt.push_str("\n\n");

    if let Some(arch) = architecture {
        prompt.push_str("=== CURRENT ARCHITECTURE ===\n");
        prompt.push_str(arch.trim());
        prompt.push_str("\n\n");
    }

    prompt.push_str("=== OPEN ISSUES ===\n");
    for issue in issues {
        let body = truncate(&issue.body, 300);
        let labels = if issue.labels.is_empty() {
            String::new()
        } else {
            format!(" [labels: {}]", issue.labels.join(", "))
        };
        prompt.push_str(&format!(
            "#{}: {}{}\n{}\n\n",
            issue.number, issue.title, labels, body
        ));
    }

    prompt.push_str(
        "=== INSTRUCTIONS ===\n\
         For each issue above, decide:\n\
         - CLOSE: conflicts with vision, stated non-goal, or superseded by a merged feature\n\
         - ADJUST: valid but needs label or priority changes\n\
         - KEEP: aligns with vision, still relevant\n\
         - MERGE: duplicates another open issue (specify which)\n\
         \n\
         Output ONLY a JSON array. No explanation before or after it.\n\
         Required format:\n\
         [\n\
           {\"number\": 107, \"action\": \"CLOSE\", \"reason\": \"Non-goal per VISION.md\"},\n\
           {\"number\": 108, \"action\": \"ADJUST\", \"reason\": \"P1 -> P2, not blocking\", \"labels\": [\"P2\"]},\n\
           {\"number\": 159, \"action\": \"KEEP\", \"reason\": \"P0 — foundational\"},\n\
           {\"number\": 114, \"action\": \"MERGE\", \"reason\": \"Duplicate of #159\", \"merge_into\": 159}\n\
         ]\n\
         \n\
         Every open issue MUST appear in the output. Keep reasons under 70 characters.",
    );

    prompt
}

/// Run `claude --print` with the given prompt, capture stdout.
///
/// Pipes the prompt via stdin to avoid OS argument size limits (E2BIG).
fn run_claude_capture(prompt: &str) -> Result<String> {
    let mut child = Command::new("claude")
        .args(["--print", "--dangerously-skip-permissions"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("Failed to run `claude`. Is it installed and in PATH?")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(prompt.as_bytes())
            .context("Failed to write prompt to claude stdin")?;
    }

    let output = child
        .wait_with_output()
        .context("Failed to wait for claude process")?;

    if !output.status.success() {
        bail!(
            "claude exited with non-zero status ({})",
            output.status.code().unwrap_or(-1)
        );
    }

    String::from_utf8(output.stdout).context("Claude output is not valid UTF-8")
}

/// Extract the JSON array from Claude's output and build recommendations.
pub fn parse_recommendations(output: &str, issues: &[Issue]) -> Result<Vec<Recommendation>> {
    let start = output.find('[').ok_or_else(|| {
        anyhow::anyhow!(
            "No JSON array found in Claude output:\n{}",
            &output[..output.len().min(200)]
        )
    })?;
    let end = output
        .rfind(']')
        .ok_or_else(|| anyhow::anyhow!("Malformed JSON in Claude output (no closing `]`)"))?;

    let json_str = &output[start..=end];
    let json: Value =
        serde_json::from_str(json_str).context("Failed to parse recommendations JSON")?;

    let arr = json
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Expected JSON array from Claude"))?;

    // Build a lookup from issue number → title.
    let title_map: std::collections::HashMap<u64, &str> = issues
        .iter()
        .map(|i| (i.number, i.title.as_str()))
        .collect();

    let recommendations = arr
        .iter()
        .filter_map(|item| {
            let number = item["number"].as_u64()?;
            let action_str = item["action"].as_str()?;
            let reason = item["reason"].as_str().unwrap_or("").to_string();
            let title = title_map
                .get(&number)
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("#{number}"));

            let action = match action_str {
                "CLOSE" => Action::Close,
                "ADJUST" => {
                    let labels = item["labels"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|l| l.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    Action::Adjust { labels }
                }
                "MERGE" => {
                    let into = item["merge_into"].as_u64().unwrap_or(0);
                    Action::Merge { into }
                }
                _ => Action::Keep,
            };

            Some(Recommendation {
                number,
                title,
                action,
                reason,
            })
        })
        .collect();

    Ok(recommendations)
}

/// Print the triage report to stdout.
fn display_report(recommendations: &[Recommendation]) {
    for rec in recommendations {
        let action_str = match &rec.action {
            Action::Close => "CLOSE",
            Action::Adjust { .. } => "ADJUST",
            Action::Keep => "KEEP",
            Action::Merge { .. } => "MERGE",
        };
        let title_short = truncate(&rec.title, 28);
        println!(
            "{:<6}  #{:<5}  {:<28}  {}",
            action_str, rec.number, title_short, rec.reason
        );
    }
}

/// Apply triage actions via the `gh` CLI.
fn apply_changes(owner: &str, repo: &str, recommendations: &[Recommendation]) -> Result<()> {
    let repo_flag = format!("{owner}/{repo}");

    for rec in recommendations {
        match &rec.action {
            Action::Keep => continue,

            Action::Close => {
                println!("  Closing #{}: {}", rec.number, rec.title);
                let comment = format!(
                    "Closing via `sipag triage`: {}\n\n\
                     This issue was identified as misaligned with the project vision \
                     (VISION.md) or superseded by a completed feature.",
                    rec.reason
                );
                let status = Command::new("gh")
                    .args([
                        "issue",
                        "close",
                        &rec.number.to_string(),
                        "--repo",
                        &repo_flag,
                        "--comment",
                        &comment,
                    ])
                    .status()
                    .with_context(|| format!("Failed to close #{}", rec.number))?;
                if !status.success() {
                    eprintln!("  Warning: could not close #{}", rec.number);
                }
            }

            Action::Adjust { labels } => {
                println!("  Adjusting #{}: {}", rec.number, rec.title);
                // Add labels (if any).
                if !labels.is_empty() {
                    let label_str = labels.join(",");
                    let status = Command::new("gh")
                        .args([
                            "issue",
                            "edit",
                            &rec.number.to_string(),
                            "--repo",
                            &repo_flag,
                            "--add-label",
                            &label_str,
                        ])
                        .status()
                        .with_context(|| format!("Failed to edit labels for #{}", rec.number))?;
                    if !status.success() {
                        eprintln!("  Warning: could not update labels for #{}", rec.number);
                    }
                }
                // Leave a comment explaining the adjustment.
                let comment = format!("Adjusting via `sipag triage`: {}", rec.reason);
                let _ = Command::new("gh")
                    .args([
                        "issue",
                        "comment",
                        &rec.number.to_string(),
                        "--repo",
                        &repo_flag,
                        "--body",
                        &comment,
                    ])
                    .status();
            }

            Action::Merge { into } => {
                println!(
                    "  Closing #{} as duplicate of #{}: {}",
                    rec.number, into, rec.title
                );
                let comment = format!(
                    "Closing via `sipag triage` as duplicate of #{}. {}\n\n\
                     Please track this in #{} instead.",
                    into, rec.reason, into
                );
                let status = Command::new("gh")
                    .args([
                        "issue",
                        "close",
                        &rec.number.to_string(),
                        "--repo",
                        &repo_flag,
                        "--comment",
                        &comment,
                    ])
                    .status()
                    .with_context(|| format!("Failed to close #{}", rec.number))?;
                if !status.success() {
                    eprintln!("  Warning: could not close #{}", rec.number);
                }
            }
        }
    }

    Ok(())
}

/// Truncate a string to `max_chars` Unicode characters, appending `…` if trimmed.
pub fn truncate(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{head}...")
    } else {
        head
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_repo_valid() {
        let (owner, repo) = parse_repo("Dorky-Robot/sipag").unwrap();
        assert_eq!(owner, "Dorky-Robot");
        assert_eq!(repo, "sipag");
    }

    #[test]
    fn test_parse_repo_valid_simple() {
        let (owner, repo) = parse_repo("acme/my-project").unwrap();
        assert_eq!(owner, "acme");
        assert_eq!(repo, "my-project");
    }

    #[test]
    fn test_parse_repo_no_slash() {
        assert!(parse_repo("nodomain").is_err());
    }

    #[test]
    fn test_parse_repo_empty_owner() {
        assert!(parse_repo("/repo").is_err());
    }

    #[test]
    fn test_parse_repo_empty_name() {
        assert!(parse_repo("owner/").is_err());
    }

    #[test]
    fn test_truncate_short() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_exact() {
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_long() {
        let result = truncate("hello world", 5);
        assert_eq!(result, "hello...");
    }

    #[test]
    fn test_truncate_unicode() {
        // Each emoji is one char
        let s = "🦀🦀🦀🦀🦀";
        assert_eq!(truncate(s, 3), "🦀🦀🦀...");
    }

    #[test]
    fn test_parse_recommendations_basic() {
        let issues = vec![
            Issue {
                number: 107,
                title: "Cost tracking".to_string(),
                body: String::new(),
                labels: vec![],
            },
            Issue {
                number: 159,
                title: "Centralized state".to_string(),
                body: String::new(),
                labels: vec![],
            },
        ];

        let json = r#"[
          {"number": 107, "action": "CLOSE", "reason": "Non-goal per VISION.md"},
          {"number": 159, "action": "KEEP", "reason": "P0 — foundational"}
        ]"#;

        let recs = parse_recommendations(json, &issues).unwrap();
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].number, 107);
        assert!(matches!(recs[0].action, Action::Close));
        assert_eq!(recs[0].reason, "Non-goal per VISION.md");
        assert_eq!(recs[1].number, 159);
        assert!(matches!(recs[1].action, Action::Keep));
    }

    #[test]
    fn test_parse_recommendations_adjust_with_labels() {
        let issues = vec![Issue {
            number: 108,
            title: "Stale PR detection".to_string(),
            body: String::new(),
            labels: vec!["P1".to_string()],
        }];

        let json = r#"[
          {"number": 108, "action": "ADJUST", "reason": "P1 -> P2", "labels": ["P2"]}
        ]"#;

        let recs = parse_recommendations(json, &issues).unwrap();
        assert_eq!(recs.len(), 1);
        match &recs[0].action {
            Action::Adjust { labels } => assert_eq!(labels, &["P2"]),
            other => panic!("Expected Adjust, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_recommendations_merge() {
        let issues = vec![
            Issue {
                number: 114,
                title: "logs --follow".to_string(),
                body: String::new(),
                labels: vec![],
            },
            Issue {
                number: 159,
                title: "Centralized state".to_string(),
                body: String::new(),
                labels: vec![],
            },
        ];

        let json = r#"[
          {"number": 114, "action": "MERGE", "reason": "Superseded by #159", "merge_into": 159},
          {"number": 159, "action": "KEEP", "reason": "P0 — foundational"}
        ]"#;

        let recs = parse_recommendations(json, &issues).unwrap();
        assert_eq!(recs[0].number, 114);
        match &recs[0].action {
            Action::Merge { into } => assert_eq!(*into, 159),
            other => panic!("Expected Merge, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_recommendations_unknown_action_defaults_to_keep() {
        let issues = vec![Issue {
            number: 99,
            title: "Unknown".to_string(),
            body: String::new(),
            labels: vec![],
        }];

        let json = r#"[{"number": 99, "action": "FUTURISTIC", "reason": "who knows"}]"#;
        let recs = parse_recommendations(json, &issues).unwrap();
        assert!(matches!(recs[0].action, Action::Keep));
    }

    #[test]
    fn test_parse_recommendations_with_preamble() {
        // Claude might add text before the JSON array.
        let issues = vec![Issue {
            number: 1,
            title: "Test issue".to_string(),
            body: String::new(),
            labels: vec![],
        }];

        let output = "Here are my recommendations:\n[{\"number\": 1, \"action\": \"KEEP\", \"reason\": \"Aligns\"}]";
        let recs = parse_recommendations(output, &issues).unwrap();
        assert_eq!(recs.len(), 1);
        assert!(matches!(recs[0].action, Action::Keep));
    }

    #[test]
    fn test_parse_recommendations_no_json() {
        let issues: Vec<Issue> = vec![];
        let result = parse_recommendations("No JSON here at all", &issues);
        assert!(result.is_err());
    }

    #[test]
    fn test_build_triage_prompt_contains_vision() {
        let issues = vec![Issue {
            number: 1,
            title: "Test".to_string(),
            body: "Body".to_string(),
            labels: vec![],
        }];
        let prompt = build_triage_prompt("org/repo", "My vision text", None, &issues);
        assert!(prompt.contains("org/repo"));
        assert!(prompt.contains("My vision text"));
        assert!(prompt.contains("#1"));
        assert!(prompt.contains("Test"));
    }

    #[test]
    fn test_build_triage_prompt_with_architecture() {
        let issues: Vec<Issue> = vec![];
        let prompt = build_triage_prompt("org/repo", "Vision", Some("Arch content"), &issues);
        assert!(prompt.contains("ARCHITECTURE"));
        assert!(prompt.contains("Arch content"));
    }

    #[test]
    fn test_build_triage_prompt_truncates_long_body() {
        let long_body = "x".repeat(500);
        let issues = vec![Issue {
            number: 1,
            title: "Issue".to_string(),
            body: long_body,
            labels: vec![],
        }];
        let prompt = build_triage_prompt("org/repo", "Vision", None, &issues);
        // The body appears truncated (300 chars + "...")
        assert!(prompt.contains("..."));
    }
}
