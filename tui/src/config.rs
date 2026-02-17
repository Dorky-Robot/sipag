use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[allow(dead_code)]
pub struct SipagConfig {
    pub repo: String,
    pub concurrency: u32,
    pub poll_interval: u64,
}

pub fn load_config(project_dir: &Path) -> Result<SipagConfig> {
    let config_path = project_dir.join(".sipag");
    let content = fs::read_to_string(&config_path)
        .with_context(|| format!("No .sipag found in {}", project_dir.display()))?;

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

    let repo = vars
        .get("SIPAG_REPO")
        .cloned()
        .unwrap_or_default();

    let concurrency = vars
        .get("SIPAG_CONCURRENCY")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);

    let poll_interval = vars
        .get("SIPAG_POLL_INTERVAL")
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);

    Ok(SipagConfig {
        repo,
        concurrency,
        poll_interval,
    })
}
