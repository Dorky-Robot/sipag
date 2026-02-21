use anyhow::{Context, Result};
use std::fs::{self, File};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::task::{append_ended, slugify, write_tracking_file};

fn now_timestamp() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Check that Docker daemon is running and accessible.
fn preflight_docker_running() -> Result<()> {
    let status = Command::new("docker")
        .args(["info"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => Ok(()),
        _ => anyhow::bail!(
            "Error: Docker is not running.\n\n  To fix:\n\n    Open Docker Desktop    (macOS)\n    systemctl start docker (Linux)"
        ),
    }
}

/// Check that the required Docker image exists.
fn preflight_docker_image(image: &str) -> Result<()> {
    let status = Command::new("docker")
        .args(["image", "inspect", image])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => Ok(()),
        _ => anyhow::bail!(
            "Error: Docker image '{}' not found.\n\n  To fix, run:\n\n    sipag setup\n\n  Or build manually:\n\n    docker build -t {} .",
            image,
            image
        ),
    }
}

/// Check that Claude authentication is available.
/// Checks CLAUDE_CODE_OAUTH_TOKEN env, then ~/.sipag/token (OAuth), then ANTHROPIC_API_KEY.
fn preflight_auth(sipag_dir: &Path) -> Result<()> {
    // Check CLAUDE_CODE_OAUTH_TOKEN env var
    if let Ok(token) = std::env::var("CLAUDE_CODE_OAUTH_TOKEN") {
        if !token.is_empty() {
            return Ok(());
        }
    }
    // Check ~/.sipag/token (primary OAuth method)
    let token_file = sipag_dir.join("token");
    if token_file.exists() {
        if let Ok(contents) = fs::read_to_string(&token_file) {
            if !contents.trim().is_empty() {
                return Ok(());
            }
        }
    }
    // Check ANTHROPIC_API_KEY as fallback
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        if !key.is_empty() {
            eprintln!("Note: Using ANTHROPIC_API_KEY. For OAuth instead, run:");
            eprintln!("  claude setup-token");
            eprintln!("  cp ~/.claude/token {}/token", sipag_dir.display());
            return Ok(());
        }
    }
    anyhow::bail!(
        "Error: No Claude authentication found.\n\n  To fix, run these two commands:\n\n    claude setup-token\n    cp ~/.claude/token {}/token\n\n  The first command opens your browser to authenticate with Anthropic.\n  The second copies the token to where sipag workers can use it.\n\n  Alternative: export ANTHROPIC_API_KEY=sk-ant-... (if you have an API key)",
        sipag_dir.display()
    )
}

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

    // Preflight checks: fail early with clear messages before touching any state.
    preflight_auth(sipag_dir)?;
    preflight_docker_running()?;
    preflight_docker_image(image)?;

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
        &now_timestamp(),
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
        let _ = append_ended(&tracking_file, &now_timestamp());
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
}
