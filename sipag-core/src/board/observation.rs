//! Observation — a katulong session sipag has noticed.
//!
//! When a user starts a katulong session (anywhere — directly via the
//! katulong UI, via `sipag dispatch`, etc.) the observer subsystem in
//! `sipag/src/serve/observers.rs` writes one of these to disk so the
//! board can surface it. Initially every observation is filed under
//! the synthetic `misc` project; a separate categorize worker can
//! later move it to the right Project + KR using gemma4 over the
//! session's recent activity.
//!
//! On disk: `~/.sipag/observations/<host>--<session>.toml`

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

use super::atomic_write;

/// "misc" is sipag's synthetic project for observations that haven't
/// been categorized. Real projects go under `~/.sipag/projects/<name>/`;
/// observations file under `misc` until a categorize worker moves them.
pub const MISC_PROJECT: &str = "misc";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    /// Host id from `~/.sipag/hosts.toml`.
    pub host: String,
    /// Katulong session name. Stable per-host.
    pub session: String,
    /// Katulong's session UUID. Useful when the session gets renamed.
    #[serde(default)]
    pub session_id: String,
    /// First time the observer noticed this (host, session) pair.
    pub first_seen: String,
    /// Most recent poll where the session was alive.
    pub last_seen: String,
    /// `"active"` when the session is alive on its host, `"ended"` once
    /// the host stops listing it. We never delete observations from
    /// disk — closed sessions stay around so the board can show
    /// historical activity per KR.
    pub status: String,
    /// Project this observation rolls up to. Defaults to `MISC_PROJECT`
    /// until a categorize worker moves it.
    #[serde(default = "default_project")]
    pub project: String,
    /// KR id within `project` this observation supports. `0` means
    /// "uncategorized" — sipag knows which project but not which KR.
    #[serde(default)]
    pub kr_id: u64,
    /// Free-form labels. Mirror sipag's choreography pattern: workers
    /// react to label changes (e.g., the categorize worker triggers on
    /// `triage`, surfaces ambiguous matches with `attention`).
    #[serde(default)]
    pub labels: Vec<String>,
    /// Optional one-line summary captured by the categorize worker.
    /// Empty until populated.
    #[serde(default)]
    pub summary: String,
    /// Objective-shaped KR references — what this session contributes to.
    /// Cross-cutting: one session can advance multiple KRs across multiple
    /// objectives. When non-empty, the objective-shaped UI uses this as the
    /// authoritative categorization. The legacy `project`/`kr_id` fields
    /// remain for read-only back-compat with the project-scoped layout.
    #[serde(default)]
    pub kr_refs: Vec<super::KrRef>,
}

fn default_project() -> String {
    MISC_PROJECT.to_string()
}

impl Observation {
    /// Stable identifier on disk: `<host>--<session>` with anything
    /// filesystem-hostile replaced by `_`. Two different hosts can
    /// have a session with the same name, so we always namespace by
    /// host.
    pub fn id(&self) -> String {
        Self::id_for(&self.host, &self.session)
    }

    pub fn id_for(host: &str, session: &str) -> String {
        let h = sanitize(host);
        let s = sanitize(session);
        format!("{h}--{s}")
    }

    pub fn path(sipag_dir: &Path, id: &str) -> std::path::PathBuf {
        sipag_dir.join("observations").join(format!("{id}.toml"))
    }

    pub fn load(sipag_dir: &Path, id: &str) -> Result<Self> {
        let path = Self::path(sipag_dir, id);
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("observation '{id}' not found at {}", path.display()))?;
        let obs: Self = toml::from_str(&content)
            .with_context(|| format!("invalid TOML in {}", path.display()))?;
        Ok(obs)
    }

    pub fn save(&self, sipag_dir: &Path) -> Result<()> {
        let dir = sipag_dir.join("observations");
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("create observations dir {}", dir.display()))?;
        let path = dir.join(format!("{}.toml", self.id()));
        let content = toml::to_string_pretty(self).context("serialize observation")?;
        atomic_write(&path, content.as_bytes())
    }

    /// List all observations on disk. Optional `project_filter` returns
    /// only those rolled up under that project (use `MISC_PROJECT` to
    /// see uncategorized).
    pub fn list(sipag_dir: &Path, project_filter: Option<&str>) -> Result<Vec<Self>> {
        let dir = sipag_dir.join("observations");
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&dir)
            .with_context(|| format!("read observations dir {}", dir.display()))?
            .flatten()
        {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            let content = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(_) => continue,
            };
            match toml::from_str::<Self>(&content) {
                Ok(obs) => {
                    if let Some(p) = project_filter {
                        if obs.project != p {
                            continue;
                        }
                    }
                    out.push(obs);
                }
                Err(_) => continue,
            }
        }
        out.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
        Ok(out)
    }
}

/// Replace anything that is not safe for a single-segment filename
/// with `_`. Defensive — katulong sessions are usually plain ASCII
/// alphanumeric, but we don't want to inherit bugs from a weird name.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fresh_obs() -> Observation {
        Observation {
            host: "prime".into(),
            session: "passkey-link".into(),
            session_id: "abc-123".into(),
            first_seen: "2026-04-29T00:00:00Z".into(),
            last_seen: "2026-04-29T00:01:00Z".into(),
            status: "active".into(),
            project: MISC_PROJECT.into(),
            kr_id: 0,
            labels: vec![],
            summary: String::new(),
            kr_refs: vec![],
        }
    }

    #[test]
    fn id_namespaces_by_host() {
        let a = Observation::id_for("prime", "claude");
        let b = Observation::id_for("mini", "claude");
        assert_ne!(a, b);
        assert_eq!(a, "prime--claude");
        assert_eq!(b, "mini--claude");
    }

    #[test]
    fn id_sanitizes_unsafe_chars() {
        let id = Observation::id_for("prime", "weird/path:name");
        assert!(!id.contains('/'));
        assert!(!id.contains(':'));
    }

    #[test]
    fn round_trip_disk() {
        let dir = TempDir::new().unwrap();
        let obs = fresh_obs();
        obs.save(dir.path()).unwrap();
        let loaded = Observation::load(dir.path(), &obs.id()).unwrap();
        assert_eq!(loaded.host, obs.host);
        assert_eq!(loaded.project, MISC_PROJECT);
        assert_eq!(loaded.session_id, "abc-123");
    }

    #[test]
    fn list_filters_by_project() {
        let dir = TempDir::new().unwrap();
        let mut a = fresh_obs();
        a.session = "one".into();
        a.save(dir.path()).unwrap();
        let mut b = fresh_obs();
        b.session = "two".into();
        b.project = "agent-manager".into();
        b.save(dir.path()).unwrap();

        let misc = Observation::list(dir.path(), Some(MISC_PROJECT)).unwrap();
        assert_eq!(misc.len(), 1);
        assert_eq!(misc[0].session, "one");

        let am = Observation::list(dir.path(), Some("agent-manager")).unwrap();
        assert_eq!(am.len(), 1);
        assert_eq!(am[0].session, "two");

        let all = Observation::list(dir.path(), None).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn defaults_when_fields_missing() {
        // Older observations without `project` / `kr_id` / `labels` /
        // `summary` should still load.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("observations/legacy.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"
host = "prime"
session = "x"
first_seen = "2026-04-29T00:00:00Z"
last_seen = "2026-04-29T00:00:00Z"
status = "active"
"#,
        )
        .unwrap();

        let obs = Observation::load(dir.path(), "legacy").unwrap();
        assert_eq!(obs.project, MISC_PROJECT);
        assert_eq!(obs.kr_id, 0);
        assert!(obs.labels.is_empty());
        assert!(obs.summary.is_empty());
    }
}
