//! HTTP client for katulong's crew session API.
//!
//! Uses `curl` via `std::process::Command` for HTTP requests, consistent
//! with how sipag already calls `docker` and `gh`.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Session status returned by the katulong API.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionStatus {
    pub name: String,
    #[serde(default)]
    pub running: bool,
    #[serde(default)]
    pub has_child_processes: bool,
}

/// Session info returned by the list endpoint.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionInfo {
    pub name: String,
    #[serde(default)]
    pub running: bool,
}

/// Remote connection config from `~/.katulong/remote.json`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RemoteConfig {
    pub url: String,
    #[serde(rename = "apiKey")]
    pub api_key: String,
}

impl RemoteConfig {
    /// Load from `~/.katulong/remote.json`.
    pub fn load() -> Result<Self> {
        let home = std::env::var("HOME").context("HOME not set")?;
        let path = std::path::PathBuf::from(home)
            .join(".katulong")
            .join("remote.json");
        Self::load_from(&path)
    }

    /// Load from a specific path.
    pub fn load_from(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let config: Self = serde_json::from_str(&content)
            .with_context(|| format!("invalid JSON in {}", path.display()))?;
        Ok(config)
    }
}

/// HTTP client for the katulong crew API.
pub struct KatulongClient {
    url: String,
    api_key: String,
}

impl KatulongClient {
    /// Load connection details from `~/.katulong/remote.json`.
    pub fn from_remote_json() -> Result<Self> {
        let config = RemoteConfig::load()?;
        Ok(Self::new(config.url, config.api_key))
    }

    /// Create a client from explicit URL and API key.
    pub fn new(url: String, api_key: String) -> Self {
        // Normalize: strip trailing slash from URL.
        let url = url.trim_end_matches('/').to_string();
        Self { url, api_key }
    }

    /// POST /sessions — create or find an existing session.
    ///
    /// The katulong API is idempotent: if a session with this name already
    /// exists it returns it rather than erroring.
    pub fn create_session(&self, name: &str) -> Result<()> {
        let body = serde_json::json!({ "name": name });
        let url = format!("{}/sessions", self.url);

        let output = curl_post(&url, &self.api_key, &body.to_string())?;

        // Accept 200 or 201.
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            anyhow::bail!(
                "Failed to create session '{name}': {}\n{}",
                stderr.trim(),
                stdout.trim()
            );
        }
        Ok(())
    }

    /// POST /sessions/:name/exec — send a command to a session.
    pub fn exec_session(&self, name: &str, input: &str) -> Result<()> {
        let body = serde_json::json!({ "input": input });
        let url = format!("{}/sessions/{}/exec", self.url, name);

        let output = curl_post(&url, &self.api_key, &body.to_string())?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            anyhow::bail!(
                "Failed to exec in session '{name}': {}\n{}",
                stderr.trim(),
                stdout.trim()
            );
        }
        Ok(())
    }

    /// GET /sessions/:name/status — check session status.
    pub fn session_status(&self, name: &str) -> Result<SessionStatus> {
        let url = format!("{}/sessions/{}/status", self.url, name);

        let output = curl_get(&url, &self.api_key)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "Failed to get status for session '{name}': {}",
                stderr.trim()
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let status: SessionStatus = serde_json::from_str(&stdout)
            .with_context(|| format!("invalid JSON from session status: {stdout}"))?;
        Ok(status)
    }

    /// GET /sessions — list all sessions.
    pub fn list_sessions(&self) -> Result<Vec<SessionInfo>> {
        let url = format!("{}/sessions", self.url);

        let output = curl_get(&url, &self.api_key)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to list sessions: {}", stderr.trim());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let sessions: Vec<SessionInfo> = serde_json::from_str(&stdout)
            .with_context(|| format!("invalid JSON from sessions list: {stdout}"))?;
        Ok(sessions)
    }

    /// DELETE /sessions/:name — kill a session.
    pub fn kill_session(&self, name: &str) -> Result<()> {
        let url = format!("{}/sessions/{}", self.url, name);

        let output = Command::new("curl")
            .args([
                "-s",
                "-X",
                "DELETE",
                "-H",
                &format!("Authorization: Bearer {}", self.api_key),
                &url,
            ])
            .output()
            .context("failed to run curl")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to kill session '{name}': {}", stderr.trim());
        }
        Ok(())
    }

    /// Return the base URL (for display/diagnostics).
    pub fn url(&self) -> &str {
        &self.url
    }
}

// ── curl helpers ────────────────────────────────────────────────────────────

fn curl_post(url: &str, api_key: &str, body: &str) -> Result<std::process::Output> {
    Command::new("curl")
        .args([
            "-s",
            "-X",
            "POST",
            "-H",
            "Content-Type: application/json",
            "-H",
            &format!("Authorization: Bearer {}", api_key),
            "-d",
            body,
            url,
        ])
        .output()
        .context("failed to run curl")
}

fn curl_get(url: &str, api_key: &str) -> Result<std::process::Output> {
    Command::new("curl")
        .args([
            "-s",
            "-H",
            &format!("Authorization: Bearer {}", api_key),
            url,
        ])
        .output()
        .context("failed to run curl")
}

// ── Dispatch logic ──────────────────────────────────────────────────────────

/// Session naming convention: `{project}--{role}`.
pub fn session_name(project: &str, role: &str) -> String {
    format!("{project}--{role}")
}

/// Generate the worktree path for a task within a project container.
pub fn worktree_path(project: &str, task_id: u64) -> String {
    format!("/work/{project}/.worktrees/task-{task_id}")
}

/// Generate the worktree branch name for a task.
pub fn worktree_branch(task_id: u64) -> String {
    format!("fix/task-{task_id}")
}

/// Generate the git worktree add command.
pub fn worktree_command(project: &str, task_id: u64) -> String {
    let branch = worktree_branch(task_id);
    format!("cd /work/{project} && git worktree add .worktrees/task-{task_id} -b {branch}")
}

/// Generate the agent launch command for a task.
pub fn agent_command(
    project: &str,
    task_id: u64,
    title: &str,
    role_command: &str,
    use_worktree: bool,
) -> String {
    let work_dir = if use_worktree {
        worktree_path(project, task_id)
    } else {
        format!("/work/{project}")
    };
    format!("cd {work_dir} && {role_command} -p 'Work on task #{task_id}: {title}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_name_format() {
        assert_eq!(session_name("katulong", "dev"), "katulong--dev");
        assert_eq!(session_name("kubo", "test"), "kubo--test");
    }

    #[test]
    fn worktree_path_format() {
        assert_eq!(
            worktree_path("katulong", 42),
            "/work/katulong/.worktrees/task-42"
        );
    }

    #[test]
    fn worktree_branch_format() {
        assert_eq!(worktree_branch(42), "fix/task-42");
    }

    #[test]
    fn worktree_command_format() {
        let cmd = worktree_command("katulong", 42);
        assert!(cmd.contains("cd /work/katulong"));
        assert!(cmd.contains("git worktree add .worktrees/task-42 -b fix/task-42"));
    }

    #[test]
    fn agent_command_with_worktree() {
        let cmd = agent_command("katulong", 42, "Fix auth bug", "yolo", true);
        assert!(cmd.contains("cd /work/katulong/.worktrees/task-42"));
        assert!(cmd.contains("yolo -p 'Work on task #42: Fix auth bug'"));
    }

    #[test]
    fn agent_command_without_worktree() {
        let cmd = agent_command("katulong", 42, "Run tests", "yolo", false);
        assert!(cmd.contains("cd /work/katulong &&"));
        assert!(!cmd.contains("worktrees"));
    }

    #[test]
    fn remote_config_load_from_valid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote.json");
        std::fs::write(
            &path,
            r#"{"url": "https://example.com", "apiKey": "test-key"}"#,
        )
        .unwrap();

        let config = RemoteConfig::load_from(&path).unwrap();
        assert_eq!(config.url, "https://example.com");
        assert_eq!(config.api_key, "test-key");
    }

    #[test]
    fn remote_config_load_from_missing_file() {
        let result = RemoteConfig::load_from(std::path::Path::new("/nonexistent/remote.json"));
        assert!(result.is_err());
    }

    #[test]
    fn client_strips_trailing_slash() {
        let client = KatulongClient::new("https://example.com/".to_string(), "key".to_string());
        assert_eq!(client.url(), "https://example.com");
    }
}
