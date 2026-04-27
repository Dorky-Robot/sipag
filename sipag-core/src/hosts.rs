//! Multi-host config for the agent-manager mesh.
//!
//! `~/.sipag/hosts.toml` lists the katulong instances sipag is allowed to
//! talk to. Each entry is `{ id, url, apiKey }`. The API key values come
//! from each host's own `~/.katulong/remote.json`; sipag loads them into
//! memory at startup and never exposes them to the browser.
//!
//! ```toml
//! [[host]]
//! id     = "mini"
//! url    = "https://katulong-mini.felixflor.es"
//! apiKey = "..."
//! ```

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::config::default_sipag_dir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Host {
    pub id: String,
    pub url: String,
    #[serde(rename = "apiKey")]
    pub api_key: String,
}

impl Host {
    /// URL with any trailing slash stripped so endpoint concatenation
    /// produces a well-formed URL regardless of how the user wrote it.
    pub fn base_url(&self) -> &str {
        self.url.trim_end_matches('/')
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HostsConfig {
    #[serde(default, rename = "host")]
    pub hosts: Vec<Host>,
}

impl HostsConfig {
    /// Load `<sipag_dir>/hosts.toml`. If the file is missing, return an
    /// empty config — the server should log a hint pointing at the
    /// example, not refuse to start.
    pub fn load() -> Result<Self> {
        Self::load_from(&default_hosts_path())
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let cfg: Self = toml::from_str(&content)
            .with_context(|| format!("invalid TOML in {}", path.display()))?;
        Ok(cfg)
    }

    pub fn find(&self, id: &str) -> Option<&Host> {
        self.hosts.iter().find(|h| h.id == id)
    }
}

pub fn default_hosts_path() -> PathBuf {
    default_sipag_dir().join("hosts.toml")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parses_three_host_config() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("hosts.toml");
        std::fs::write(
            &path,
            r#"
[[host]]
id = "mini"
url = "https://katulong-mini.felixflor.es/"
apiKey = "k1"

[[host]]
id = "prime"
url = "https://katulong-prime.felixflor.es"
apiKey = "k2"

[[host]]
id = "og"
url = "https://katulong-og.felixflor.es"
apiKey = "k3"
"#,
        )
        .unwrap();

        let cfg = HostsConfig::load_from(&path).unwrap();
        assert_eq!(cfg.hosts.len(), 3);
        assert_eq!(
            cfg.find("mini").unwrap().base_url(),
            "https://katulong-mini.felixflor.es"
        );
        assert_eq!(cfg.find("prime").unwrap().api_key, "k2");
        assert!(cfg.find("missing").is_none());
    }

    #[test]
    fn missing_file_yields_empty_config() {
        let tmp = TempDir::new().unwrap();
        let cfg = HostsConfig::load_from(&tmp.path().join("nope.toml")).unwrap();
        assert!(cfg.hosts.is_empty());
    }
}
