use anyhow::Result;
use std::process::{Command, Stdio};

/// Find a working timeout command: `timeout` (Linux/coreutils) or `gtimeout` (macOS Homebrew).
/// Returns `None` if neither is available.
pub fn resolve_timeout_command() -> Option<String> {
    for bin in ["timeout", "gtimeout"] {
        if Command::new(bin)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return Some(bin.to_string());
        }
    }
    None
}

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
            "Docker is not running.\n\n  To fix:\n\n    Open Docker Desktop    (macOS)\n    systemctl start docker (Linux)"
        ),
    }
}

/// Check whether a Docker container with the given name is currently running.
pub fn is_container_running(container_name: &str) -> bool {
    Command::new("docker")
        .args([
            "ps",
            "--filter",
            &format!("name=^{container_name}$"),
            "--format",
            "{{.Names}}",
        ])
        .output()
        .map(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty())
        .unwrap_or(false)
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
            "Docker image '{}' not found.\n\n  To fix:\n\n    docker pull {}\n\n  Or build locally:\n\n    docker build -t {} .",
            image, image, image
        ),
    }
}
