use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::auth;
use crate::docker;
use crate::prompt;
use crate::task::{append_ended, write_tracking_file};

pub use docker::RunConfig;

fn now_timestamp() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Run a task in a Docker container.
///
/// Orchestrates: auth preflight → Docker preflight → write tracking file →
/// build prompt → resolve token → execute container → finalize lifecycle.
///
/// When `cfg.background` is true the binary re-invokes itself via the hidden
/// `_bg-exec` subcommand so that file moves happen even after the caller exits.
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
    auth::preflight_auth(sipag_dir)?;
    docker::preflight_docker_running()?;
    docker::preflight_docker_image(image)?;

    let running_dir = sipag_dir.join("running");
    let tracking_file = running_dir.join(format!("{task_id}.md"));
    let container_name = format!("sipag-{task_id}");

    write_tracking_file(
        &tracking_file,
        repo_url,
        issue,
        &container_name,
        description,
        &now_timestamp(),
    )?;

    let prompt_text = prompt::build_prompt(description, "", issue);

    if background {
        // Re-invoke the binary with an internal subcommand so that the
        // post-completion file moves happen even after the parent exits.
        let exe = std::env::current_exe().unwrap_or_else(|_| "sipag".into());
        let mut cmd = Command::new(&exe);
        cmd.args([
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
        ]);
        if let Some(issue_num) = issue {
            cmd.args(["--issue", issue_num]);
        }
        cmd.spawn().context("Failed to spawn background worker")?;
    } else {
        let token = auth::resolve_token(sipag_dir);
        exec_and_finalize(
            sipag_dir,
            task_id,
            repo_url,
            &prompt_text,
            image,
            timeout_secs,
            token.as_deref(),
        )?;
    }

    Ok(())
}

/// Internal background worker: runs Docker and handles file moves on completion.
///
/// Called by `run_impl` when `background=true` via the hidden `_bg-exec`
/// subcommand.  The tracking file is already written by the parent call.
pub fn run_bg_exec(
    sipag_dir: &Path,
    task_id: &str,
    repo_url: &str,
    description: &str,
    issue: Option<&str>,
    image: &str,
    timeout_secs: u64,
) -> Result<()> {
    let prompt_text = prompt::build_prompt(description, "", issue);
    let token = auth::resolve_token(sipag_dir);
    exec_and_finalize(
        sipag_dir,
        task_id,
        repo_url,
        &prompt_text,
        image,
        timeout_secs,
        token.as_deref(),
    )
}

/// Run the Docker container and move tracking files to done/ or failed/.
///
/// This is the shared core used by both the foreground and background paths,
/// eliminating the previous copy-paste duplication between `run_impl` and
/// `run_bg_exec`.
fn exec_and_finalize(
    sipag_dir: &Path,
    task_id: &str,
    repo_url: &str,
    prompt_text: &str,
    image: &str,
    timeout_secs: u64,
    token: Option<&str>,
) -> Result<()> {
    let running_dir = sipag_dir.join("running");
    let done_dir = sipag_dir.join("done");
    let failed_dir = sipag_dir.join("failed");
    let tracking_file = running_dir.join(format!("{task_id}.md"));
    let log_path = running_dir.join(format!("{task_id}.log"));
    let container_name = format!("sipag-{task_id}");

    let success = docker::run_container(
        &container_name,
        repo_url,
        prompt_text,
        image,
        timeout_secs,
        token,
        &log_path,
    );

    if let Err(e) = append_ended(&tracking_file, &now_timestamp()) {
        eprintln!(
            "sipag: failed to append ended timestamp to {}: {e}",
            tracking_file.display()
        );
    }

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

/// Run claude directly (non-Docker mode, for the `next` command).
pub fn run_claude(title: &str, body: &str) -> Result<()> {
    let mut prompt_text = title.to_string();
    if let Ok(prefix) = std::env::var("SIPAG_PROMPT_PREFIX") {
        prompt_text = format!("{prefix}\n\n{prompt_text}");
    }
    if !body.is_empty() {
        prompt_text.push_str(&format!("\n\n{body}"));
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
    args.push(prompt_text);

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
