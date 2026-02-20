use anyhow::Result;
use chrono::Utc;
use std::fs;
use std::io::Write as IoWrite;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::config::Config;
use crate::repo;
use crate::task;

/// Build the Claude prompt for a task, matching the bash executor exactly.
pub fn build_prompt(title: &str, body: &str, issue: Option<&str>) -> String {
    let mut prompt = String::new();
    prompt.push_str("You are working on the repository at /work.\n");
    prompt.push_str("\nYour task:\n");
    prompt.push_str(title);
    prompt.push('\n');
    if !body.is_empty() {
        prompt.push_str(body);
        prompt.push('\n');
    }
    prompt.push_str("\nInstructions:\n");
    prompt.push_str("- Create a new branch with a descriptive name\n");
    prompt.push_str("- Before writing any code, open a draft pull request with this body:\n");
    prompt.push_str(
        "    > This PR is being worked on by sipag. Commits will appear as work progresses.\n",
    );
    prompt.push_str(&format!("    Task: {}\n", title));
    if let Some(iss) = issue {
        prompt.push_str(&format!("    Issue: #{}\n", iss));
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

/// Resolve the OAuth token: env var takes precedence, then token file.
fn resolve_token(config: &Config) -> Option<String> {
    std::env::var("CLAUDE_CODE_OAUTH_TOKEN")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            if config.token_file.exists() {
                fs::read_to_string(&config.token_file)
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            } else {
                None
            }
        })
}

/// Bash script run inside the Docker container.
const CONTAINER_SCRIPT: &str = r#"git clone "$REPO_URL" /work && cd /work
git config user.name "sipag"
git config user.email "sipag@localhost"
claude --print --dangerously-skip-permissions -p "$PROMPT""#;

/// Build the `docker run` Command for a task, without running it.
fn docker_command(
    container_name: &str,
    repo_url: &str,
    prompt: &str,
    config: &Config,
    log_path: &Path,
) -> Result<Command> {
    let log_file = fs::File::create(log_path)?;
    let log_file2 = log_file.try_clone()?;

    let mut cmd = Command::new("timeout");
    cmd.arg(config.timeout.to_string());
    cmd.args(["docker", "run", "--rm", "--name", container_name]);
    cmd.args(["-e", "CLAUDE_CODE_OAUTH_TOKEN"]);
    cmd.args(["-e", "GH_TOKEN"]);
    cmd.arg("-e").arg(format!("REPO_URL={}", repo_url));
    cmd.arg("-e").arg(format!("PROMPT={}", prompt));
    cmd.arg(&config.image);
    cmd.args(["bash", "-c", CONTAINER_SCRIPT]);
    cmd.stdout(Stdio::from(log_file));
    cmd.stderr(Stdio::from(log_file2));

    Ok(cmd)
}

/// Append `ended: <timestamp>` to a tracking file.
fn append_ended(tracking_file: &Path) {
    let ended = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    if let Ok(mut f) = fs::OpenOptions::new().append(true).open(tracking_file) {
        let _ = writeln!(f, "ended: {}", ended);
    }
}

/// Move a task (and optional log) to the destination directory.
fn move_task(task_id: &str, src_dir: &Path, dst_dir: &Path) {
    let src_md = src_dir.join(format!("{}.md", task_id));
    let dst_md = dst_dir.join(format!("{}.md", task_id));
    if src_md.exists() {
        let _ = fs::rename(&src_md, &dst_md);
    }

    let src_log = src_dir.join(format!("{}.log", task_id));
    let dst_log = dst_dir.join(format!("{}.log", task_id));
    if src_log.exists() {
        let _ = fs::rename(&src_log, &dst_log);
    }
}

/// Core implementation for `sipag run`.
///
/// Creates a tracking file in `running/`, launches Docker (foreground or background),
/// then moves the task to `done/` or `failed/` when complete.
pub fn run_impl(
    task_id: &str,
    repo_url: &str,
    description: &str,
    issue: Option<&str>,
    background: bool,
    config: &Config,
) -> Result<()> {
    let running_dir = config.sipag_dir.join("running");
    let done_dir = config.sipag_dir.join("done");
    let failed_dir = config.sipag_dir.join("failed");
    let tracking_file = running_dir.join(format!("{}.md", task_id));
    let log_file = running_dir.join(format!("{}.log", task_id));
    let container_name = format!("sipag-{}", task_id);

    // Write tracking file
    let started = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let mut tracking_content = format!("---\nrepo: {}\n", repo_url);
    if let Some(iss) = issue {
        tracking_content.push_str(&format!("issue: {}\n", iss));
    }
    tracking_content.push_str(&format!("started: {}\n", started));
    tracking_content.push_str(&format!("container: {}\n", container_name));
    tracking_content.push_str("---\n");
    tracking_content.push_str(description);
    tracking_content.push('\n');
    fs::write(&tracking_file, &tracking_content)?;

    // Build prompt
    let prompt = build_prompt(description, "", issue);

    // Ensure token is in env for Docker to inherit
    if let Some(token) = resolve_token(config) {
        // SAFETY: single-threaded at this point; we're just forwarding the credential
        unsafe {
            std::env::set_var("CLAUDE_CODE_OAUTH_TOKEN", &token);
        }
    }

    if background {
        let config_clone = config.clone();
        let task_id_owned = task_id.to_string();
        let repo_url_owned = repo_url.to_string();
        let prompt_owned = prompt.clone();
        let container_name_owned = container_name.clone();
        let running_dir_clone = running_dir.clone();
        let done_dir_clone = done_dir.clone();
        let failed_dir_clone = failed_dir.clone();
        let log_file_clone = log_file.clone();
        let tracking_file_clone = tracking_file.clone();

        std::thread::spawn(move || {
            let mut cmd = match docker_command(
                &container_name_owned,
                &repo_url_owned,
                &prompt_owned,
                &config_clone,
                &log_file_clone,
            ) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error building docker command: {}", e);
                    return;
                }
            };

            let success = cmd.status().map(|s| s.success()).unwrap_or(false);

            append_ended(&tracking_file_clone);

            if success {
                move_task(&task_id_owned, &running_dir_clone, &done_dir_clone);
                println!("==> Done: {}", task_id_owned);
            } else {
                move_task(&task_id_owned, &running_dir_clone, &failed_dir_clone);
                println!("==> Failed: {}", task_id_owned);
            }
        });
    } else {
        let mut cmd = docker_command(
            &container_name,
            repo_url,
            &prompt,
            config,
            &log_file,
        )?;

        let status = cmd.status()?;

        append_ended(&tracking_file);

        if status.success() {
            move_task(task_id, &running_dir, &done_dir);
            println!("==> Done: {}", task_id);
        } else {
            move_task(task_id, &running_dir, &failed_dir);
            println!("==> Failed: {}", task_id);
        }
    }

    Ok(())
}

/// Worker loop: pick tasks from `queue/`, run in Docker, move to `done/` or `failed/`.
///
/// Loops until the queue is empty.
pub fn run_queue(config: &Config) -> Result<()> {
    let queue_dir = config.sipag_dir.join("queue");
    let running_dir = config.sipag_dir.join("running");
    let failed_dir = config.sipag_dir.join("failed");
    let mut processed = 0usize;

    loop {
        // Get the first .md file from queue (sorted alphabetically)
        let mut files = task::sorted_md_files(&queue_dir)?;
        files.retain(|p| p.extension().map(|x| x == "md").unwrap_or(false));

        let task_file = match files.into_iter().next() {
            Some(f) => f,
            None => {
                if processed == 0 {
                    println!("No tasks in queue — use 'sipag add' to queue a task");
                } else {
                    println!("Queue empty — processed {} task(s)", processed);
                }
                return Ok(());
            }
        };

        let task_name = task_file
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        // Parse task frontmatter
        let parsed = match task::parse_task_file(&task_file) {
            Ok(t) => t,
            Err(e) => {
                eprintln!(
                    "Error: failed to parse task file: {}: {}",
                    task_file.display(),
                    e
                );
                let _ = fs::rename(&task_file, failed_dir.join(format!("{}.md", task_name)));
                println!("==> Failed: {}", task_name);
                processed += 1;
                continue;
            }
        };

        // Look up repo URL
        let url = match repo::repo_url(&parsed.repo, &config.sipag_dir) {
            Ok(u) => u,
            Err(_) => {
                eprintln!(
                    "Error: repo '{}' not found in repos.conf",
                    parsed.repo
                );
                let _ = fs::rename(&task_file, failed_dir.join(format!("{}.md", task_name)));
                println!("==> Failed: {}", task_name);
                processed += 1;
                continue;
            }
        };

        // Extract issue number from source (e.g. "github#142" -> "142")
        let issue_num: Option<String> = parsed.source.as_ref().and_then(|s| {
            s.rfind('#').map(|i| s[i + 1..].to_string())
        });

        // Move task to running/ (run_impl will overwrite with tracking metadata)
        let running_path = running_dir.join(format!("{}.md", task_name));
        let _ = fs::rename(&task_file, &running_path);

        let _ = run_impl(
            &task_name,
            &url,
            &parsed.title,
            issue_num.as_deref(),
            false,
            config,
        );

        processed += 1;
    }
}

/// Kill a running container and move the task to `failed/`.
pub fn kill_task(task_id: &str, config: &Config) -> Result<()> {
    let tracking_file = config.sipag_dir.join("running").join(format!("{}.md", task_id));

    if !tracking_file.exists() {
        anyhow::bail!("task '{}' not found in running/", task_id);
    }

    let container_name = format!("sipag-{}", task_id);

    // Kill the container (ignore errors if already stopped)
    let _ = Command::new("docker")
        .args(["kill", &container_name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    let running_dir = config.sipag_dir.join("running");
    let failed_dir = config.sipag_dir.join("failed");
    move_task(task_id, &running_dir, &failed_dir);

    println!("Killed: {}", task_id);
    Ok(())
}

/// Show queue state: for each of queue/running/done/failed, list files and counts.
pub fn print_status(sipag_dir: &Path) -> Result<()> {
    for (label, subdir) in &[
        ("Queue", "queue"),
        ("Running", "running"),
        ("Done", "done"),
        ("Failed", "failed"),
    ] {
        let dir = sipag_dir.join(subdir);
        let mut items: Vec<String> = if dir.is_dir() {
            fs::read_dir(&dir)?
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        } else {
            vec![]
        };
        items.sort();

        if !items.is_empty() {
            println!("{} ({}):", label, items.len());
            for item in &items {
                println!("  {}", item);
            }
        }
    }
    Ok(())
}

/// List tasks from running/, done/, failed/ in tabular form.
pub fn print_ps(sipag_dir: &Path) -> Result<()> {
    println!(
        "{:<44}  {:<8}  {:<10}  REPO",
        "ID", "STATUS", "DURATION"
    );

    let now = Utc::now();
    let mut found = false;

    for dir_status in &["running", "done", "failed"] {
        let dir = sipag_dir.join(dir_status);
        if !dir.is_dir() {
            continue;
        }

        let mut files: Vec<PathBuf> = fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "md").unwrap_or(false))
            .collect();
        files.sort();

        for file in files {
            let task_id = file
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            let info = match task::parse_tracking_file(&file) {
                Ok(i) => i,
                Err(_) => continue,
            };

            let duration = compute_duration(&info.started, &info.ended, &now);
            let repo_display = if info.repo.is_empty() {
                "unknown".to_string()
            } else {
                info.repo.clone()
            };

            println!(
                "{:<44}  {:<8}  {:<10}  {}",
                &task_id[..task_id.len().min(44)],
                dir_status,
                duration,
                repo_display,
            );
            found = true;
        }
    }

    if !found {
        println!("No tasks found.");
    }

    Ok(())
}

fn compute_duration(
    started: &Option<String>,
    ended: &Option<String>,
    now: &chrono::DateTime<Utc>,
) -> String {
    let start = match started {
        Some(s) => match s.parse::<chrono::DateTime<Utc>>() {
            Ok(dt) => dt,
            Err(_) => return "-".to_string(),
        },
        None => return "-".to_string(),
    };

    let end = match ended {
        Some(e) => match e.parse::<chrono::DateTime<Utc>>() {
            Ok(dt) => dt,
            Err(_) => *now,
        },
        None => *now,
    };

    let secs = (end - start).num_seconds().max(0);
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Print task file and log for a named task.
pub fn show_task(name: &str, sipag_dir: &Path) -> Result<()> {
    for dir_status in &["queue", "running", "done", "failed"] {
        let candidate = sipag_dir.join(dir_status).join(format!("{}.md", name));
        if candidate.exists() {
            println!("=== Task: {} ===", name);
            println!("Status: {}", dir_status);
            let content = fs::read_to_string(&candidate)?;
            print!("{}", content);

            let log_file = sipag_dir.join(dir_status).join(format!("{}.log", name));
            if log_file.exists() {
                println!("=== Log ===");
                let log = fs::read_to_string(&log_file)?;
                print!("{}", log);
            }

            return Ok(());
        }
    }

    anyhow::bail!("task '{}' not found", name)
}

/// Print the log for a task.
pub fn print_logs(task_id: &str, sipag_dir: &Path) -> Result<()> {
    for dir_status in &["running", "done", "failed"] {
        let log_file = sipag_dir.join(dir_status).join(format!("{}.log", task_id));
        if log_file.exists() {
            let content = fs::read_to_string(&log_file)?;
            print!("{}", content);
            return Ok(());
        }
    }

    anyhow::bail!("no log found for task '{}'", task_id)
}

/// Move a task from `failed/` back to `queue/` for retry.
pub fn retry_task(name: &str, sipag_dir: &Path) -> Result<()> {
    let failed_file = sipag_dir.join("failed").join(format!("{}.md", name));
    if !failed_file.exists() {
        anyhow::bail!("task '{}' not found in failed/", name);
    }

    let queue_file = sipag_dir.join("queue").join(format!("{}.md", name));
    fs::rename(&failed_file, &queue_file)?;

    // Remove the old log if present
    let log_file = sipag_dir.join("failed").join(format!("{}.log", name));
    if log_file.exists() {
        let _ = fs::remove_file(&log_file);
    }

    println!("Retrying: {} (moved to queue)", name);
    Ok(())
}
