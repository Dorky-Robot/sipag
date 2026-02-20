use anyhow::{Context, Result};
use std::fs::{self, File};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::task::{append_ended, slugify, write_tracking_file};

/// Build the Claude prompt for a task.
pub fn build_prompt(title: &str, body: &str, issue: Option<&str>) -> String {
    let mut prompt = format!("You are working on the repository at /work.\n\nYour task:\n{title}\n");
    if !body.is_empty() {
        prompt.push_str(body);
        prompt.push('\n');
    }
    prompt.push_str("\nInstructions:\n");
    prompt.push_str("- Create a new branch with a descriptive name\n");
    prompt.push_str("- Before writing any code, open a draft pull request with this body:\n");
    prompt.push_str(&format!(
        "    > This PR is being worked on by sipag. Commits will appear as work progresses.\n    Task: {title}\n"
    ));
    if let Some(iss) = issue {
        prompt.push_str(&format!("    Issue: #{iss}\n"));
    }
    prompt.push_str("- The PR title should match the task title\n");
    prompt.push_str("- Commit after each logical unit of work (not just at the end)\n");
    prompt.push_str("- Push after each commit so GitHub reflects progress in real time\n");
    prompt.push_str("- Run any existing tests and make sure they pass\n");
    prompt.push_str(
        "- When all work is complete, update the PR body with a summary of what changed and why\n",
    );
    prompt.push_str("- When all work is complete, mark the pull request as ready for review\n");
    prompt
}

/// Configuration for running a task in a Docker container.
pub struct RunConfig<'a> {
    pub task_id: &'a str,
    pub repo_url: &'a str,
    pub description: &'a str,
    pub issue: Option<&'a str>,
    pub background: bool,
    pub image: &'a str,
    pub timeout_secs: u64,
}

/// Run a task in a Docker container. Blocks until the container exits.
/// Moves tracking file + log to done/ or failed/ based on exit code.
pub fn run_impl(sipag_dir: &Path, cfg: RunConfig<'_>) -> Result<()> {
    let RunConfig {
        task_id,
        repo_url,
        description,
        issue,
        background,
        image,
        timeout_secs,
    } = cfg;
    let running_dir = sipag_dir.join("running");
    let done_dir = sipag_dir.join("done");
    let failed_dir = sipag_dir.join("failed");
    let tracking_file = running_dir.join(format!("{task_id}.md"));
    let log_path = running_dir.join(format!("{task_id}.log"));
    let container_name = format!("sipag-{task_id}");

    // Write tracking file
    write_tracking_file(
        &tracking_file,
        repo_url,
        issue,
        &container_name,
        description,
    )?;

    let prompt = build_prompt(description, "", issue);

    // Resolve OAuth token from file if not in environment
    let mut oauth_token = std::env::var("CLAUDE_CODE_OAUTH_TOKEN").ok();
    if oauth_token.is_none() {
        if let Ok(home) = std::env::var("HOME") {
            let token_file = Path::new(&home).join(".sipag/token");
            if token_file.exists() {
                oauth_token = fs::read_to_string(&token_file)
                    .ok()
                    .map(|s| s.trim().to_string());
            }
        }
    }

    let bash_script = r#"git clone "$REPO_URL" /work && cd /work
git config user.name "sipag"
git config user.email "sipag@localhost"
claude --print --dangerously-skip-permissions -p "$PROMPT""#;

    let run_docker = |log_file: &Path| -> bool {
        let log_out = match File::create(log_file) {
            Ok(f) => f,
            Err(_) => return false,
        };
        let log_err = match log_out.try_clone() {
            Ok(f) => f,
            Err(_) => return false,
        };

        let mut cmd = Command::new("timeout");
        cmd.arg(timeout_secs.to_string())
            .arg("docker")
            .arg("run")
            .arg("--rm")
            .arg("--name")
            .arg(&container_name)
            .arg("-e")
            .arg("CLAUDE_CODE_OAUTH_TOKEN")
            .arg("-e")
            .arg("GH_TOKEN")
            .arg("-e")
            .arg(format!("REPO_URL={repo_url}"))
            .arg("-e")
            .arg(format!("PROMPT={prompt}"))
            .arg(image)
            .arg("bash")
            .arg("-c")
            .arg(bash_script)
            .stdout(Stdio::from(log_out))
            .stderr(Stdio::from(log_err));

        if let Some(ref token) = oauth_token {
            cmd.env("CLAUDE_CODE_OAUTH_TOKEN", token);
        }

        cmd.status().map(|s| s.success()).unwrap_or(false)
    };

    if background {
        // Spawn a subprocess to manage the container lifecycle.
        // We re-invoke the binary with an internal subcommand so the
        // post-completion file moves happen even after the parent exits.
        let exe = std::env::current_exe().unwrap_or_else(|_| "sipag".into());
        Command::new(&exe)
            .args([
                "_bg-exec",
                "--task-id",
                task_id,
                "--repo-url",
                repo_url,
                "--description",
                description,
                "--image",
                image,
                "--timeout",
                &timeout_secs.to_string(),
                "--sipag-dir",
                &sipag_dir.to_string_lossy(),
            ])
            .spawn()
            .context("Failed to spawn background worker")?;
    } else {
        let success = run_docker(&log_path);
        let _ = append_ended(&tracking_file);
        if success {
            if tracking_file.exists() {
                fs::rename(&tracking_file, done_dir.join(format!("{task_id}.md")))?;
            }
            if log_path.exists() {
                fs::rename(&log_path, done_dir.join(format!("{task_id}.log")))?;
            }
            println!("==> Done: {task_id}");
        } else {
            if tracking_file.exists() {
                fs::rename(&tracking_file, failed_dir.join(format!("{task_id}.md")))?;
            }
            if log_path.exists() {
                fs::rename(&log_path, failed_dir.join(format!("{task_id}.log")))?;
            }
            println!("==> Failed: {task_id}");
        }
    }

    Ok(())
}

/// Internal background worker: runs Docker and handles file moves on completion.
/// Called by run_impl when background=true.
pub fn run_bg_exec(
    sipag_dir: &Path,
    task_id: &str,
    repo_url: &str,
    description: &str,
    image: &str,
    timeout_secs: u64,
) -> Result<()> {
    let running_dir = sipag_dir.join("running");
    let done_dir = sipag_dir.join("done");
    let failed_dir = sipag_dir.join("failed");
    let tracking_file = running_dir.join(format!("{task_id}.md"));
    let log_path = running_dir.join(format!("{task_id}.log"));
    let container_name = format!("sipag-{task_id}");

    let prompt = build_prompt(description, "", None);

    let mut oauth_token = std::env::var("CLAUDE_CODE_OAUTH_TOKEN").ok();
    if oauth_token.is_none() {
        if let Ok(home) = std::env::var("HOME") {
            let token_file = Path::new(&home).join(".sipag/token");
            if token_file.exists() {
                oauth_token = fs::read_to_string(&token_file)
                    .ok()
                    .map(|s| s.trim().to_string());
            }
        }
    }

    let bash_script = r#"git clone "$REPO_URL" /work && cd /work
git config user.name "sipag"
git config user.email "sipag@localhost"
claude --print --dangerously-skip-permissions -p "$PROMPT""#;

    let log_out = File::create(&log_path).context("Failed to create log file")?;
    let log_err = log_out.try_clone()?;

    let mut cmd = Command::new("timeout");
    cmd.arg(timeout_secs.to_string())
        .arg("docker")
        .arg("run")
        .arg("--rm")
        .arg("--name")
        .arg(&container_name)
        .arg("-e")
        .arg("CLAUDE_CODE_OAUTH_TOKEN")
        .arg("-e")
        .arg("GH_TOKEN")
        .arg("-e")
        .arg(format!("REPO_URL={repo_url}"))
        .arg("-e")
        .arg(format!("PROMPT={prompt}"))
        .arg(image)
        .arg("bash")
        .arg("-c")
        .arg(bash_script)
        .stdout(Stdio::from(log_out))
        .stderr(Stdio::from(log_err));

    if let Some(ref token) = oauth_token {
        cmd.env("CLAUDE_CODE_OAUTH_TOKEN", token);
    }

    let success = cmd.status().map(|s| s.success()).unwrap_or(false);
    let _ = append_ended(&tracking_file);

    if success {
        if tracking_file.exists() {
            fs::rename(&tracking_file, done_dir.join(format!("{task_id}.md")))?;
        }
        if log_path.exists() {
            fs::rename(&log_path, done_dir.join(format!("{task_id}.log")))?;
        }
        println!("==> Done: {task_id}");
    } else {
        if tracking_file.exists() {
            fs::rename(&tracking_file, failed_dir.join(format!("{task_id}.md")))?;
        }
        if log_path.exists() {
            fs::rename(&log_path, failed_dir.join(format!("{task_id}.log")))?;
        }
        println!("==> Failed: {task_id}");
    }

    Ok(())
}

/// Build the Claude prompt for an interactive PR review session.
///
/// `repo` is the `owner/name` identifier of the GitHub repository.
/// `prs_json` is the raw JSON output from `gh pr list`.
pub fn start_build_review_prompt(repo: &str, prs_json: &str) -> String {
    format!(
        r#"You are facilitating a PR review session for {repo}.

Open PRs:
{prs_json}

YOUR ROLE:
- Summarize the open PRs at a high level — group by size/type
- Ask the human about their review priorities and risk tolerance
- Discuss trade-offs for non-trivial PRs
- For simple/clean PRs, propose batch approvals
- When the human agrees, apply reviews via gh pr review
- You can: approve, request changes, or comment on PRs
- You can fetch full diffs on demand with: gh pr diff <number> --repo {repo}

Keep it conversational. Start with a high-level summary and ask where to focus."#
    )
}

/// Start an interactive PR review session for `repo` (owner/name format).
///
/// Fetches open PRs via `gh pr list`, builds a review prompt, and launches
/// `claude` in interactive mode so the human can discuss and apply reviews.
pub fn run_review(repo: &str) -> Result<()> {
    println!("Fetching open PRs for {repo}...");
    let pr_output = Command::new("gh")
        .args([
            "pr",
            "list",
            "--repo",
            repo,
            "--state",
            "open",
            "--json",
            "number,title,body,files,reviewDecision,additions,deletions",
            "--limit",
            "20",
        ])
        .output()
        .context("Failed to run 'gh pr list' — is the gh CLI installed and authenticated?")?;

    if !pr_output.status.success() {
        let stderr = String::from_utf8_lossy(&pr_output.stderr);
        anyhow::bail!("gh pr list failed: {}", stderr.trim());
    }

    let prs_json = String::from_utf8_lossy(&pr_output.stdout);
    let prompt = start_build_review_prompt(repo, prs_json.trim());

    // Launch claude interactively (no --print so it stays in chat mode).
    let mut args = vec!["--dangerously-skip-permissions".to_string()];
    if let Ok(model) = std::env::var("SIPAG_MODEL") {
        args.push("--model".to_string());
        args.push(model);
    }
    if let Ok(extra) = std::env::var("SIPAG_CLAUDE_ARGS") {
        for arg in extra.split_whitespace() {
            args.push(arg.to_string());
        }
    }
    args.push("-p".to_string());
    args.push(prompt);

    let status = Command::new("claude")
        .args(&args)
        .status()
        .context("Failed to run claude — is it installed?")?;

    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("claude exited with non-zero status: {}", status)
    }
}

/// Run claude directly (non-Docker mode, for the `next` command).
pub fn run_claude(title: &str, body: &str) -> Result<()> {
    let mut prompt = title.to_string();
    if let Ok(prefix) = std::env::var("SIPAG_PROMPT_PREFIX") {
        prompt = format!("{prefix}\n\n{prompt}");
    }
    if !body.is_empty() {
        prompt.push_str(&format!("\n\n{body}"));
    }

    let mut args = vec!["--print".to_string()];
    let skip_perms = std::env::var("SIPAG_SKIP_PERMISSIONS").unwrap_or_else(|_| "1".to_string());
    if skip_perms == "1" {
        args.push("--dangerously-skip-permissions".to_string());
    }
    if let Ok(model) = std::env::var("SIPAG_MODEL") {
        args.push("--model".to_string());
        args.push(model);
    }
    if let Ok(extra) = std::env::var("SIPAG_CLAUDE_ARGS") {
        for arg in extra.split_whitespace() {
            args.push(arg.to_string());
        }
    }
    args.push("-p".to_string());
    args.push(prompt);

    let timeout = std::env::var("SIPAG_TIMEOUT")
        .unwrap_or_else(|_| "600".to_string())
        .parse::<u64>()
        .unwrap_or(600);

    let status = Command::new("timeout")
        .arg(timeout.to_string())
        .arg("claude")
        .args(&args)
        .status()
        .context("Failed to run claude")?;

    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("claude exited with non-zero status: {}", status)
    }
}

/// Generate a task ID from the current timestamp and a slugified description.
pub fn generate_task_id(description: &str) -> String {
    let slug = slugify(description);
    let ts = chrono::Utc::now().format("%Y%m%d%H%M%S");
    let truncated = slug.get(..30.min(slug.len())).unwrap_or(&slug);
    let id = format!("{ts}-{truncated}");
    id.trim_end_matches('-').to_string()
}

/// Format a duration in seconds as human-readable string.
pub fn format_duration(secs: i64) -> String {
    if secs < 0 {
        return "-".to_string();
    }
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_prompt_basic() {
        let prompt = build_prompt("Fix the bug", "", None);
        assert!(prompt.contains("Fix the bug"));
        assert!(prompt.contains("repository at /work"));
        assert!(prompt.contains("pull request"));
    }

    #[test]
    fn test_build_prompt_with_issue() {
        let prompt = build_prompt("Fix the bug", "", Some("42"));
        assert!(prompt.contains("Issue: #42"));
    }

    #[test]
    fn test_build_prompt_with_body() {
        let prompt = build_prompt("Fix the bug", "Some detailed body", None);
        assert!(prompt.contains("Some detailed body"));
    }

    #[test]
    fn test_build_prompt_no_issue() {
        let prompt = build_prompt("Fix the bug", "", None);
        assert!(!prompt.contains("Issue: #"));
    }

    #[test]
    fn test_format_duration_seconds() {
        assert_eq!(format_duration(30), "30s");
    }

    #[test]
    fn test_format_duration_minutes() {
        assert_eq!(format_duration(90), "1m30s");
    }

    #[test]
    fn test_format_duration_hours() {
        assert_eq!(format_duration(3661), "1h1m");
    }

    #[test]
    fn test_format_duration_negative() {
        assert_eq!(format_duration(-1), "-");
    }

    #[test]
    fn test_generate_task_id() {
        let id = generate_task_id("Fix the authentication bug");
        assert!(id.contains("fix-the-authentication-bug"));
        // Should start with timestamp (14 digits)
        assert!(id.chars().take(14).all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn test_start_build_review_prompt_contains_repo() {
        let prompt = start_build_review_prompt("acme/backend", "[]");
        assert!(prompt.contains("acme/backend"));
    }

    #[test]
    fn test_start_build_review_prompt_contains_prs_json() {
        let prs = r#"[{"number":42,"title":"Add caching","additions":150,"deletions":10}]"#;
        let prompt = start_build_review_prompt("acme/backend", prs);
        assert!(prompt.contains(prs));
    }

    #[test]
    fn test_start_build_review_prompt_role_instructions() {
        let prompt = start_build_review_prompt("acme/backend", "[]");
        assert!(prompt.contains("gh pr review"));
        assert!(prompt.contains("gh pr diff"));
        assert!(prompt.contains("batch"));
    }

    #[test]
    fn test_start_build_review_prompt_diff_command_includes_repo() {
        let prompt = start_build_review_prompt("myorg/myrepo", "[]");
        assert!(prompt.contains("gh pr diff <number> --repo myorg/myrepo"));
    }
}
