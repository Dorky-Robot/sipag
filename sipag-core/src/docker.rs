use anyhow::Result;
use std::fs::File;
use std::path::Path;
use std::process::{Command, Stdio};

/// Check that Docker daemon is running and accessible.
pub fn preflight_docker_running() -> Result<()> {
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

/// Check that the required Docker image exists locally.
pub fn preflight_docker_image(image: &str) -> Result<()> {
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

const BASH_SCRIPT: &str = r#"git clone "$REPO_URL" /work && cd /work
git config user.name "sipag"
git config user.email "sipag@localhost"
claude --print --dangerously-skip-permissions -p "$PROMPT""#;

/// Run a Docker container and stream output to `log_path`.
///
/// Returns `true` if the container exited successfully, `false` otherwise.
pub fn run_container(
    container_name: &str,
    repo_url: &str,
    prompt: &str,
    image: &str,
    timeout_secs: u64,
    oauth_token: Option<&str>,
    log_path: &Path,
) -> bool {
    let log_out = match File::create(log_path) {
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
        .arg(container_name)
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
        .arg(BASH_SCRIPT)
        .stdout(Stdio::from(log_out))
        .stderr(Stdio::from(log_err));

    if let Some(token) = oauth_token {
        cmd.env("CLAUDE_CODE_OAUTH_TOKEN", token);
    }

    cmd.status().map(|s| s.success()).unwrap_or(false)
}
