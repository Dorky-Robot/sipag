use anyhow::{bail, Context, Result};
use std::fs;
use std::io::Write;
use std::path::Path;

/// Look up a repo name in repos.conf and return its URL.
pub fn get_repo_url(sipag_dir: &Path, name: &str) -> Result<String> {
    let conf = sipag_dir.join("repos.conf");
    if !conf.exists() {
        bail!("No repos registered. Use: sipag repo add <name> <url>");
    }
    let content =
        fs::read_to_string(&conf).with_context(|| format!("Failed to read {}", conf.display()))?;
    for line in content.lines() {
        if line.trim_start().starts_with('#') {
            continue;
        }
        if let Some((key, val)) = line.split_once('=') {
            if key.trim() == name {
                return Ok(val.trim().to_string());
            }
        }
    }
    bail!("Repo '{}' not found in repos.conf", name)
}

/// Register a new repo name → URL mapping in repos.conf.
pub fn add_repo(sipag_dir: &Path, name: &str, url: &str) -> Result<()> {
    let conf = sipag_dir.join("repos.conf");
    if conf.exists() {
        let content = fs::read_to_string(&conf)
            .with_context(|| format!("Failed to read {}", conf.display()))?;
        for line in content.lines() {
            if line.trim_start().starts_with('#') {
                continue;
            }
            if let Some((key, _)) = line.split_once('=') {
                if key.trim() == name {
                    bail!("Error: repo '{}' already exists", name);
                }
            }
        }
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&conf)
        .with_context(|| format!("Failed to open {}", conf.display()))?;
    writeln!(file, "{}={}", name, url)?;
    Ok(())
}

/// List all repos from repos.conf as (name, url) pairs.
pub fn list_repos(sipag_dir: &Path) -> Result<Vec<(String, String)>> {
    let conf = sipag_dir.join("repos.conf");
    if !conf.exists() {
        return Ok(Vec::new());
    }
    let content =
        fs::read_to_string(&conf).with_context(|| format!("Failed to read {}", conf.display()))?;
    let repos = content
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .filter_map(|line| {
            line.split_once('=')
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        })
        .collect();
    Ok(repos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_add_and_get_repo() {
        let dir = TempDir::new().unwrap();
        add_repo(dir.path(), "myrepo", "https://github.com/org/repo").unwrap();
        let url = get_repo_url(dir.path(), "myrepo").unwrap();
        assert_eq!(url, "https://github.com/org/repo");
    }

    #[test]
    fn test_get_repo_not_found() {
        let dir = TempDir::new().unwrap();
        assert!(get_repo_url(dir.path(), "nonexistent").is_err());
    }

    #[test]
    fn test_get_repo_no_conf_file() {
        let dir = TempDir::new().unwrap();
        // No repos.conf exists
        assert!(get_repo_url(dir.path(), "anyrepo").is_err());
    }

    #[test]
    fn test_add_duplicate_repo() {
        let dir = TempDir::new().unwrap();
        add_repo(dir.path(), "myrepo", "https://github.com/org/repo").unwrap();
        assert!(add_repo(dir.path(), "myrepo", "https://github.com/org/other").is_err());
    }

    #[test]
    fn test_list_repos() {
        let dir = TempDir::new().unwrap();
        add_repo(dir.path(), "repo1", "https://github.com/org/repo1").unwrap();
        add_repo(dir.path(), "repo2", "https://github.com/org/repo2").unwrap();
        let repos = list_repos(dir.path()).unwrap();
        assert_eq!(repos.len(), 2);
        assert_eq!(repos[0].0, "repo1");
        assert_eq!(repos[0].1, "https://github.com/org/repo1");
        assert_eq!(repos[1].0, "repo2");
    }

    #[test]
    fn test_list_repos_empty() {
        let dir = TempDir::new().unwrap();
        let repos = list_repos(dir.path()).unwrap();
        assert!(repos.is_empty());
    }

    #[test]
    fn test_list_repos_skips_comments() {
        let dir = TempDir::new().unwrap();
        let conf = dir.path().join("repos.conf");
        fs::write(
            &conf,
            "# This is a comment\nrepo1=https://github.com/org/repo1\n# another comment\nrepo2=https://github.com/org/repo2\n",
        )
        .unwrap();
        let repos = list_repos(dir.path()).unwrap();
        assert_eq!(repos.len(), 2);
        assert_eq!(repos[0].0, "repo1");
        assert_eq!(repos[1].0, "repo2");
    }

    #[test]
    fn test_get_repo_url_skips_comments() {
        let dir = TempDir::new().unwrap();
        let conf = dir.path().join("repos.conf");
        fs::write(
            &conf,
            "# myrepo=https://old-url.com\nmyrepo=https://github.com/org/repo\n",
        )
        .unwrap();
        let url = get_repo_url(dir.path(), "myrepo").unwrap();
        assert_eq!(url, "https://github.com/org/repo");
    }
}
