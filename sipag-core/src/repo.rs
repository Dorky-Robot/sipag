use anyhow::{bail, Context, Result};
use std::fs;
use std::io::Write as IoWrite;
use std::path::Path;

/// Look up a repo URL by name from `repos.conf`.
///
/// File format: one `name=url` entry per line.
/// Returns an error if the name is not found.
pub fn repo_url(name: &str, sipag_dir: &Path) -> Result<String> {
    let conf = sipag_dir.join("repos.conf");

    if !conf.exists() {
        bail!("repos.conf not found (use 'sipag repo add' to register a repo)");
    }

    let content = fs::read_to_string(&conf)
        .with_context(|| format!("failed to read {}", conf.display()))?;

    for line in content.lines() {
        if line.is_empty() {
            continue;
        }
        // Split on first '=' only so URLs with '=' in query params are preserved
        if let Some(eq_pos) = line.find('=') {
            let key = &line[..eq_pos];
            let val = &line[eq_pos + 1..];
            if key == name {
                return Ok(val.to_string());
            }
        }
    }

    bail!("repo '{}' not found in repos.conf", name)
}

/// Register a new repo name → URL mapping.
///
/// Returns an error if the name is already registered.
pub fn repo_add(name: &str, url: &str, sipag_dir: &Path) -> Result<()> {
    let conf = sipag_dir.join("repos.conf");

    // Check for duplicate
    if conf.exists() {
        let content = fs::read_to_string(&conf)?;
        for line in content.lines() {
            if let Some(eq_pos) = line.find('=') {
                if &line[..eq_pos] == name {
                    bail!("repo '{}' already exists", name);
                }
            }
        }
    }

    // Ensure parent directory exists
    if let Some(parent) = conf.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&conf)
        .with_context(|| format!("failed to open {}", conf.display()))?;

    writeln!(f, "{}={}", name, url)?;
    Ok(())
}

/// List all registered repos as `(name, url)` pairs.
pub fn repo_list(sipag_dir: &Path) -> Result<Vec<(String, String)>> {
    let conf = sipag_dir.join("repos.conf");

    if !conf.exists() {
        return Ok(vec![]);
    }

    let content = fs::read_to_string(&conf)?;
    let mut repos = Vec::new();

    for line in content.lines() {
        if line.is_empty() {
            continue;
        }
        if let Some(eq_pos) = line.find('=') {
            let name = line[..eq_pos].to_string();
            let url = line[eq_pos + 1..].to_string();
            repos.push((name, url));
        }
    }

    Ok(repos)
}
