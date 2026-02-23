//! Git remote resolution — map local directories to GitHub repos.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A local directory resolved to its GitHub repository.
#[derive(Debug, Clone)]
pub struct ResolvedRepo {
    pub owner: String,
    pub name: String,
    pub full_name: String,
    pub local_path: PathBuf,
}

/// Resolve a local directory to a GitHub repo via its git remotes.
///
/// Checks `origin` first, then falls back to the first available remote.
pub fn resolve_repo(dir: &Path) -> Result<ResolvedRepo> {
    let dir = dir
        .canonicalize()
        .with_context(|| format!("directory does not exist: {}", dir.display()))?;

    // Try `origin` first.
    if let Ok(url) = git_remote_url(&dir, "origin") {
        if let Some((owner, name)) = parse_github_remote(&url) {
            return Ok(ResolvedRepo {
                full_name: format!("{owner}/{name}"),
                owner,
                name,
                local_path: dir,
            });
        }
    }

    // Fall back to first remote.
    let remotes = git_remote_list(&dir)?;
    for remote in &remotes {
        if let Ok(url) = git_remote_url(&dir, remote) {
            if let Some((owner, name)) = parse_github_remote(&url) {
                return Ok(ResolvedRepo {
                    full_name: format!("{owner}/{name}"),
                    owner,
                    name,
                    local_path: dir,
                });
            }
        }
    }

    bail!(
        "No GitHub remote found in {}. Is this a git repo with a GitHub remote?",
        dir.display()
    )
}

/// Get the URL for a named git remote.
fn git_remote_url(dir: &Path, remote: &str) -> Result<String> {
    let output = Command::new("git")
        .args(["-C", &dir.to_string_lossy(), "remote", "get-url", remote])
        .output()
        .context("failed to run git")?;

    if !output.status.success() {
        bail!("git remote get-url {remote} failed");
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// List all git remotes for a directory.
fn git_remote_list(dir: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["-C", &dir.to_string_lossy(), "remote"])
        .output()
        .context("failed to run git remote")?;

    if !output.status.success() {
        bail!("not a git repository: {}", dir.display());
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

/// Parse a GitHub `owner/repo` from a remote URL.
///
/// Supports:
/// - SSH:   `git@github.com:owner/repo.git`
/// - HTTPS: `https://github.com/owner/repo.git`
/// - HTTPS without `.git`: `https://github.com/owner/repo`
fn parse_github_remote(url: &str) -> Option<(String, String)> {
    let url = url.trim();

    // SSH format: git@github.com:owner/repo.git
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        let rest = rest.strip_suffix(".git").unwrap_or(rest);
        return split_owner_repo(rest);
    }

    // HTTPS format: https://github.com/owner/repo[.git]
    for prefix in &["https://github.com/", "http://github.com/"] {
        if let Some(rest) = url.strip_prefix(prefix) {
            let rest = rest.strip_suffix(".git").unwrap_or(rest);
            return split_owner_repo(rest);
        }
    }

    None
}

fn split_owner_repo(s: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = s.splitn(3, '/').collect();
    if parts.len() >= 2 && !parts[0].is_empty() && !parts[1].is_empty() {
        Some((parts[0].to_string(), parts[1].to_string()))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ssh_format() {
        assert_eq!(
            parse_github_remote("git@github.com:Dorky-Robot/sipag.git"),
            Some(("Dorky-Robot".to_string(), "sipag".to_string()))
        );
    }

    #[test]
    fn parse_ssh_without_git_suffix() {
        assert_eq!(
            parse_github_remote("git@github.com:owner/repo"),
            Some(("owner".to_string(), "repo".to_string()))
        );
    }

    #[test]
    fn parse_https_format() {
        assert_eq!(
            parse_github_remote("https://github.com/Dorky-Robot/sipag.git"),
            Some(("Dorky-Robot".to_string(), "sipag".to_string()))
        );
    }

    #[test]
    fn parse_https_without_git_suffix() {
        assert_eq!(
            parse_github_remote("https://github.com/owner/repo"),
            Some(("owner".to_string(), "repo".to_string()))
        );
    }

    #[test]
    fn parse_invalid_url_returns_none() {
        assert_eq!(parse_github_remote("not-a-url"), None);
        assert_eq!(parse_github_remote("https://gitlab.com/owner/repo"), None);
        assert_eq!(parse_github_remote(""), None);
    }

    #[test]
    fn parse_trims_whitespace() {
        assert_eq!(
            parse_github_remote("  git@github.com:owner/repo.git  \n"),
            Some(("owner".to_string(), "repo".to_string()))
        );
    }
}
