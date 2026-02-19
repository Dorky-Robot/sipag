use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Clone)]
#[allow(dead_code)]
pub struct SipagConfig {
    pub repo: String,
    pub concurrency: u32,
    pub poll_interval: u64,
    pub max_workers: u32,
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct ProjectConfig {
    pub slug: String,
    pub source: String,
    pub repo: String,
    pub concurrency: u32,
}

pub fn load_global_config(sipag_home: &Path) -> Result<SipagConfig> {
    let config_path = sipag_home.join("config");
    let vars = if config_path.exists() {
        parse_config_file(&config_path)?
    } else {
        HashMap::new()
    };

    Ok(SipagConfig {
        repo: vars.get("SIPAG_REPO").cloned().unwrap_or_default(),
        concurrency: vars
            .get("SIPAG_CONCURRENCY")
            .and_then(|v| v.parse().ok())
            .unwrap_or(2),
        poll_interval: vars
            .get("SIPAG_POLL_INTERVAL")
            .and_then(|v| v.parse().ok())
            .unwrap_or(60),
        max_workers: vars
            .get("SIPAG_MAX_WORKERS")
            .and_then(|v| v.parse().ok())
            .unwrap_or(8),
    })
}

#[allow(dead_code)]
pub fn load_project_config(project_dir: &Path) -> Result<ProjectConfig> {
    let config_path = project_dir.join("config");
    let vars = parse_config_file(&config_path)?;
    let slug = project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    Ok(ProjectConfig {
        slug,
        source: vars.get("SIPAG_SOURCE").cloned().unwrap_or_else(|| "github".to_string()),
        repo: vars.get("SIPAG_REPO").cloned().unwrap_or_default(),
        concurrency: vars
            .get("SIPAG_CONCURRENCY")
            .and_then(|v| v.parse().ok())
            .unwrap_or(2),
    })
}

/// Legacy: load config from a .sipag file in a project directory
pub fn load_legacy_config(project_dir: &Path) -> Result<SipagConfig> {
    let config_path = project_dir.join(".sipag");
    let vars = parse_config_file(&config_path)?;

    Ok(SipagConfig {
        repo: vars.get("SIPAG_REPO").cloned().unwrap_or_default(),
        concurrency: vars
            .get("SIPAG_CONCURRENCY")
            .and_then(|v| v.parse().ok())
            .unwrap_or(2),
        poll_interval: vars
            .get("SIPAG_POLL_INTERVAL")
            .and_then(|v| v.parse().ok())
            .unwrap_or(60),
        max_workers: vars
            .get("SIPAG_MAX_WORKERS")
            .and_then(|v| v.parse().ok())
            .unwrap_or(8),
    })
}

fn parse_config_file(path: &Path) -> Result<HashMap<String, String>> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read config: {}", path.display()))?;

    let mut vars = HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, val)) = line.split_once('=') {
            let val = val.trim_matches('"').trim_matches('\'');
            vars.insert(key.trim().to_string(), val.to_string());
        }
    }
    Ok(vars)
}
