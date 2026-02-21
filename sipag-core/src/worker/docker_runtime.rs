use anyhow::Result;
use std::process::{Command, Stdio};

use super::ports::ContainerRuntime;

/// Concrete adapter: checks container status using the `docker ps` CLI.
pub struct DockerCliRuntime;

impl ContainerRuntime for DockerCliRuntime {
    fn is_running(&self, container_name: &str) -> Result<bool> {
        let out = Command::new("docker")
            .args([
                "ps",
                "--filter",
                &format!("name=^{container_name}$"),
                "--format",
                "{{.Names}}",
            ])
            .stderr(Stdio::null())
            .output();

        match out {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout);
                Ok(stdout.lines().any(|l| l.trim() == container_name))
            }
            Err(_) => Ok(false),
        }
    }
}
