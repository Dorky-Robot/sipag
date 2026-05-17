//! Per-feature markdown+frontmatter store (the dispatch feature store).
//!
//! # ⛔ Deprecated 2026-05-17
//!
//! This module implements the **kanban-shaped refinement pipeline**
//! (`raw → grouped → refined → ticket`) that has been replaced by the
//! **Experimentation context** (spike → observe → iterate) per the
//! work-model reframe documented in `docs/modules.md` §3 and the project
//! memory `project-sipag-work-model-experimentation`. The thesis behind
//! deprecation: with cheap AI spikes, the cheapest specification of work
//! is to *try it and observe the result*, not to refine an upfront
//! ticket. Refinement-as-a-pipeline assumed tasks were expensive enough
//! to need batch upfront specification — that premise no longer holds.
//!
//! The source is **deliberately preserved**, not deleted, per the
//! `feedback-deprecate-with-rationale` memory: the diwa-indexed codebase
//! becomes a discoverable trail of "we tried this and moved away from
//! it." Familiar paths are easy to fall back into; an explicit
//! deprecation marker prevents future sessions from re-walking this one.
//!
//! All wiring has been stripped (CLI subcommands removed). The only
//! remaining intra-crate user is [`crate::refine`], which is also
//! deprecated together with this module.
//!
//! ## On-disk data
//!
//! Pre-existing feature files at `~/.sipag/projects/<project>/features/f-*.md`
//! are **left on disk** (sipag does not auto-migrate or delete them) but are
//! **no longer reachable from the CLI** — `sipag feature add | list | show`
//! and `sipag refine` are gone. If you have files there from an older
//! install, read them directly with your editor; the on-disk format is
//! plain markdown + YAML frontmatter (intentionally byte-compatible with
//! katulong's `dispatch-store.js`).
//!
//! ---
//!
//! ## Original docs (preserved for archaeology)
//!
//! Each feature lives in its own file under
//! `{sipag_dir}/projects/{project}/features/f-<uuid>.md`.
//!
//! The on-disk file format is intentionally **byte-compatible** with the JS
//! implementation in `katulong/lib/dispatch-store.js` so the two stores can
//! coexist during the v4 transition. The frontmatter parser/serializer in
//! this module mirror that JS code line-for-line.
//!
//! State machine: `raw` -> `grouped` -> `refined` / `needs-info`.

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::board::atomic_write;

// ── FrontValue ───────────────────────────────────────────────────────────────

/// One frontmatter scalar. Mirrors the value types the JS parser produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrontValue {
    Null,
    Bool(bool),
    Int(i64),
    String(String),
    Array(Vec<String>),
}

impl FrontValue {
    /// Render this value as it would appear after `key: ` on a frontmatter
    /// line. Mirrors the JS `toMarkdown` value branch.
    fn to_yaml_value(&self) -> String {
        match self {
            FrontValue::Null => "null".to_string(),
            FrontValue::Bool(b) => b.to_string(),
            FrontValue::Int(n) => n.to_string(),
            FrontValue::String(s) => s.clone(),
            FrontValue::Array(items) => format!("[{}]", items.join(", ")),
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            FrontValue::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[String]> {
        match self {
            FrontValue::Array(a) => Some(a.as_slice()),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            FrontValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            FrontValue::Int(n) => Some(*n),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, FrontValue::Null)
    }
}

// ── Frontmatter parse / serialize ────────────────────────────────────────────

/// Parse `---\n...\n---\n\nbody` style frontmatter.
///
/// Mirrors the JS regex in `dispatch-store.js`:
/// `/^---\n([\s\S]*?)\n---\n?([\s\S]*)$/`. Lines that don't match
/// `key: value` are skipped (matching the JS behavior). Returns `(meta, body)`
/// or `(empty, content)` when no frontmatter is present.
pub fn parse_frontmatter(content: &str) -> Result<(BTreeMap<String, FrontValue>, String)> {
    let mut meta: BTreeMap<String, FrontValue> = BTreeMap::new();

    // Equivalent of the JS regex match. We require the file to start with
    // "---\n", then read up to the next "\n---" boundary.
    let Some(rest) = content.strip_prefix("---\n") else {
        return Ok((meta, content.to_string()));
    };

    // Find the closing fence: "\n---" followed by either EOF, "\n", or end of
    // string with optional trailing newline. Match the JS lazy regex by
    // searching for the first occurrence.
    let Some(end_idx) = rest.find("\n---") else {
        // No closing fence — treat the whole thing as body, like the JS
        // regex which would also fail to match.
        return Ok((meta, content.to_string()));
    };

    let header = &rest[..end_idx];
    // After "\n---" the JS regex consumes an optional "\n".
    let after_fence = &rest[end_idx + 4..];
    let body_raw = after_fence.strip_prefix('\n').unwrap_or(after_fence);

    for line in header.split('\n') {
        let Some((key, val)) = parse_kv_line(line) else {
            continue;
        };
        let parsed = parse_scalar(val);
        meta.insert(key.to_string(), parsed);
    }

    // JS does `body.replace(/^\n+/, '')` — strip leading newlines.
    let body = body_raw.trim_start_matches('\n').to_string();

    Ok((meta, body))
}

/// Match `^(\w+):\s*(.*)$`. `\w` in JS is `[A-Za-z0-9_]`.
fn parse_kv_line(line: &str) -> Option<(&str, &str)> {
    let colon = line.find(':')?;
    let key = &line[..colon];
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let mut val = &line[colon + 1..];
    // Skip leading whitespace (\s* in the JS regex).
    val = val.trim_start();
    Some((key, val))
}

fn parse_scalar(val: &str) -> FrontValue {
    // Array: [a, b, c]
    if let Some(inner) = val.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        let items: Vec<String> = inner
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        return FrontValue::Array(items);
    }
    if val == "null" || val.is_empty() {
        return FrontValue::Null;
    }
    if val == "true" {
        return FrontValue::Bool(true);
    }
    if val == "false" {
        return FrontValue::Bool(false);
    }
    if !val.is_empty() && val.chars().all(|c| c.is_ascii_digit()) {
        if let Ok(n) = val.parse::<i64>() {
            return FrontValue::Int(n);
        }
    }
    FrontValue::String(val.to_string())
}

/// Serialize meta + body to a markdown string. Byte-compatible with the JS
/// `toMarkdown` function: `---\n` then `key: value\n` lines (in the order
/// given by the BTreeMap), then `---\n\n` then body.
pub fn serialize(meta: &BTreeMap<String, FrontValue>, body: &str) -> String {
    let mut out = String::with_capacity(64 + body.len());
    out.push_str("---\n");
    for (k, v) in meta {
        out.push_str(k);
        out.push_str(": ");
        out.push_str(&v.to_yaml_value());
        out.push('\n');
    }
    out.push_str("---\n");
    out.push('\n');
    out.push_str(body);
    out
}

// ── Feature struct ───────────────────────────────────────────────────────────

/// A dispatch feature. Mirrors the field set written by the JS store, plus
/// the optional fields the refinement engine attaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feature {
    pub id: String,
    pub status: String,
    pub projects: Option<Vec<String>>,
    pub project: Option<String>,
    pub created: String,
    pub updated: String,
    pub body: String,

    /// Set when this feature was merged into another via grouping.
    pub grouped_into: Option<String>,
    /// IDs of features that were merged into this one.
    pub source_ids: Option<Vec<String>>,
    /// Refinement payload (title/spec/subtasks/...) — opaque JSON written by
    /// Phase 3c. Stored as a string in the frontmatter under `refined:` for
    /// the JS store; modeled here as JSON to keep the format flexible.
    pub refined: Option<serde_json::Value>,
}

impl Feature {
    // ── Path helpers ─────────────────────────────────────────────────────────

    /// Directory containing feature files for a project:
    /// `{sipag_dir}/projects/{project}/features/`.
    pub fn features_dir(sipag_dir: &Path, project: &str) -> PathBuf {
        sipag_dir.join("projects").join(project).join("features")
    }

    /// Path to a specific feature file.
    pub fn path(sipag_dir: &Path, project: &str, id: &str) -> PathBuf {
        Self::features_dir(sipag_dir, project).join(format!("{id}.md"))
    }

    // ── Frontmatter <-> struct ───────────────────────────────────────────────

    fn from_meta_and_body(
        id: String,
        meta: &BTreeMap<String, FrontValue>,
        body: String,
    ) -> Result<Self> {
        let status = meta
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "raw".to_string());

        let projects = meta.get("projects").and_then(|v| match v {
            FrontValue::Array(items) => Some(items.clone()),
            FrontValue::Null => None,
            _ => None,
        });

        let project = meta
            .get("project")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let created = meta
            .get("created")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default();

        let updated = meta
            .get("updated")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default();

        let grouped_into = meta
            .get("grouped_into")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let source_ids = meta.get("source_ids").and_then(|v| match v {
            FrontValue::Array(items) => Some(items.clone()),
            _ => None,
        });

        let refined = meta.get("refined").and_then(|v| match v {
            FrontValue::String(s) => serde_json::from_str(s).ok(),
            _ => None,
        });

        Ok(Self {
            id,
            status,
            projects,
            project,
            created,
            updated,
            body,
            grouped_into,
            source_ids,
            refined,
        })
    }

    /// Build the `BTreeMap` that will be serialized to frontmatter. The JS
    /// store does `const { id, body, ...meta } = feature` and then writes
    /// every remaining key. We mimic that field set so the on-disk format
    /// stays identical: `status`, `projects`, [`project`], `created`,
    /// `updated`, plus optional refinement fields when present.
    fn to_meta(&self) -> BTreeMap<String, FrontValue> {
        let mut meta = BTreeMap::new();

        meta.insert(
            "status".to_string(),
            FrontValue::String(self.status.clone()),
        );

        meta.insert(
            "projects".to_string(),
            match &self.projects {
                Some(items) if !items.is_empty() => FrontValue::Array(items.clone()),
                _ => FrontValue::Null,
            },
        );

        if let Some(p) = &self.project {
            meta.insert("project".to_string(), FrontValue::String(p.clone()));
        }

        meta.insert(
            "created".to_string(),
            FrontValue::String(self.created.clone()),
        );
        meta.insert(
            "updated".to_string(),
            FrontValue::String(self.updated.clone()),
        );

        if let Some(g) = &self.grouped_into {
            meta.insert("grouped_into".to_string(), FrontValue::String(g.clone()));
        }
        if let Some(items) = &self.source_ids {
            meta.insert("source_ids".to_string(), FrontValue::Array(items.clone()));
        }
        if let Some(refined) = &self.refined {
            // Stored as a JSON string so it round-trips through the
            // single-line YAML scalar parser.
            meta.insert(
                "refined".to_string(),
                FrontValue::String(refined.to_string()),
            );
        }

        meta
    }

    // ── CRUD ─────────────────────────────────────────────────────────────────

    /// Create a new raw feature. Generates an `f-<uuid>` id and writes the
    /// file. Mirrors `addFeature` in the JS store.
    pub fn add(
        sipag_dir: &Path,
        project: &str,
        raw_text: &str,
        projects: Option<Vec<String>>,
    ) -> Result<Feature> {
        let id = format!("f-{}", uuid::Uuid::new_v4());
        let now = iso_now();
        let feature = Feature {
            id: id.clone(),
            status: "raw".to_string(),
            projects: projects.filter(|p| !p.is_empty()),
            project: None,
            created: now.clone(),
            updated: now,
            body: raw_text.to_string(),
            grouped_into: None,
            source_ids: None,
            refined: None,
        };
        feature.write(sipag_dir, project)?;
        Ok(feature)
    }

    /// Read a feature by id. Returns `Ok(None)` if the file doesn't exist;
    /// returns `Err` on parse failure.
    pub fn get(sipag_dir: &Path, project: &str, id: &str) -> Result<Option<Feature>> {
        let path = Self::path(sipag_dir, project, id);
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let (meta, body) = parse_frontmatter(&content)
            .with_context(|| format!("failed to parse frontmatter in {}", path.display()))?;
        let feature = Self::from_meta_and_body(id.to_string(), &meta, body)?;
        Ok(Some(feature))
    }

    /// Mutate a feature in place via a closure, refresh `updated`, and write.
    /// Returns `Ok(None)` if the feature doesn't exist.
    pub fn update<F>(
        sipag_dir: &Path,
        project: &str,
        id: &str,
        mutate: F,
    ) -> Result<Option<Feature>>
    where
        F: FnOnce(&mut Feature),
    {
        let Some(mut feature) = Self::get(sipag_dir, project, id)? else {
            return Ok(None);
        };
        mutate(&mut feature);
        feature.updated = iso_now();
        feature.write(sipag_dir, project)?;
        Ok(Some(feature))
    }

    /// Delete a feature file. Returns `false` if the file didn't exist.
    pub fn delete(sipag_dir: &Path, project: &str, id: &str) -> Result<bool> {
        let path = Self::path(sipag_dir, project, id);
        if !path.exists() {
            return Ok(false);
        }
        std::fs::remove_file(&path)
            .with_context(|| format!("failed to delete {}", path.display()))?;
        Ok(true)
    }

    /// List features in a project, optionally filtered by status. Corrupt
    /// files are logged and skipped (matching the JS `.filter(Boolean)`).
    pub fn list(
        sipag_dir: &Path,
        project: &str,
        status_filter: Option<&str>,
    ) -> Result<Vec<Feature>> {
        let dir = Self::features_dir(sipag_dir, project);
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut features = Vec::new();
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.starts_with("f-") || !name.ends_with(".md") {
                continue;
            }
            let id = name.trim_end_matches(".md").to_string();
            match Self::get(sipag_dir, project, &id) {
                Ok(Some(f)) => features.push(f),
                Ok(None) => {}
                Err(e) => log::warn!("skipping {}: {e}", path.display()),
            }
        }
        if let Some(s) = status_filter {
            features.retain(|f| f.status == s);
        }
        features.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(features)
    }

    /// All `active` features that include the given project in their
    /// `projects` list. Mirrors `getActiveByProject` in the JS store.
    pub fn get_active_by_project(sipag_dir: &Path, project: &str) -> Result<Vec<Feature>> {
        let all = Self::list(sipag_dir, project, Some("active"))?;
        Ok(all
            .into_iter()
            .filter(|f| {
                f.projects
                    .as_ref()
                    .is_some_and(|ps| ps.iter().any(|p| p == project))
            })
            .collect())
    }

    /// Append a `- HH:MM:SS text` log line to the feature body. Matches the
    /// JS `addLog` formatting (`new Date().toISOString().slice(11, 19)`).
    pub fn add_log(sipag_dir: &Path, project: &str, id: &str, text: &str) -> Result<()> {
        let updated = Self::update(sipag_dir, project, id, |f| {
            let stamp = Utc::now().format("%H:%M:%S").to_string();
            let line = format!("- {stamp} {text}");
            if f.body.is_empty() {
                f.body = line;
            } else {
                f.body.push('\n');
                f.body.push_str(&line);
            }
        })?;
        if updated.is_none() {
            return Err(anyhow!("feature {id} not found in project {project}"));
        }
        Ok(())
    }

    /// Serialize and atomically write this feature to disk. Internal — most
    /// callers should go through `add` / `update`.
    fn write(&self, sipag_dir: &Path, project: &str) -> Result<()> {
        let path = Self::path(sipag_dir, project, &self.id);
        let meta = self.to_meta();
        let content = serialize(&meta, &self.body);
        atomic_write(&path, content.as_bytes())
    }
}

fn iso_now() -> String {
    // Match `new Date().toISOString()` — millisecond precision, trailing Z.
    Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn project_name() -> &'static str {
        "demo"
    }

    #[test]
    fn parse_frontmatter_handles_all_scalar_kinds() {
        let raw = "---\n\
status: raw\n\
projects: [a, b, c]\n\
empty: \n\
nullkey: null\n\
flag: true\n\
flag2: false\n\
count: 42\n\
title: Some Title\n\
---\n\nbody text\nline 2\n";
        let (meta, body) = parse_frontmatter(raw).unwrap();
        assert_eq!(
            meta.get("status"),
            Some(&FrontValue::String("raw".to_string()))
        );
        assert_eq!(
            meta.get("projects"),
            Some(&FrontValue::Array(vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string()
            ]))
        );
        assert_eq!(meta.get("empty"), Some(&FrontValue::Null));
        assert_eq!(meta.get("nullkey"), Some(&FrontValue::Null));
        assert_eq!(meta.get("flag"), Some(&FrontValue::Bool(true)));
        assert_eq!(meta.get("flag2"), Some(&FrontValue::Bool(false)));
        assert_eq!(meta.get("count"), Some(&FrontValue::Int(42)));
        assert_eq!(
            meta.get("title"),
            Some(&FrontValue::String("Some Title".to_string()))
        );
        assert_eq!(body, "body text\nline 2\n");
    }

    #[test]
    fn parse_frontmatter_no_fence_returns_body_only() {
        let raw = "no frontmatter here\n";
        let (meta, body) = parse_frontmatter(raw).unwrap();
        assert!(meta.is_empty());
        assert_eq!(body, raw);
    }

    #[test]
    fn parse_frontmatter_strips_leading_body_newlines() {
        let raw = "---\nstatus: raw\n---\n\n\n\nbody\n";
        let (_, body) = parse_frontmatter(raw).unwrap();
        assert_eq!(body, "body\n");
    }

    #[test]
    fn serialize_round_trips_through_parse() {
        let mut meta = BTreeMap::new();
        meta.insert("status".to_string(), FrontValue::String("raw".into()));
        meta.insert(
            "projects".to_string(),
            FrontValue::Array(vec!["x".into(), "y".into()]),
        );
        meta.insert("count".to_string(), FrontValue::Int(7));
        meta.insert("flag".to_string(), FrontValue::Bool(true));
        meta.insert("nothing".to_string(), FrontValue::Null);
        let body = "hello\nworld\n";
        let s = serialize(&meta, body);
        let (parsed_meta, parsed_body) = parse_frontmatter(&s).unwrap();
        assert_eq!(parsed_meta, meta);
        assert_eq!(parsed_body, body);
    }

    #[test]
    fn serialize_starts_with_three_dashes_and_blank_line_after_close() {
        let mut meta = BTreeMap::new();
        meta.insert("status".to_string(), FrontValue::String("raw".into()));
        let s = serialize(&meta, "body");
        assert!(s.starts_with("---\nstatus: raw\n---\n\n"));
        assert!(s.ends_with("body"));
    }

    #[test]
    fn add_writes_file_with_expected_fields() {
        let dir = TempDir::new().unwrap();
        let f = Feature::add(
            dir.path(),
            project_name(),
            "do the thing",
            Some(vec!["sipag".to_string()]),
        )
        .unwrap();
        assert!(f.id.starts_with("f-"));
        assert_eq!(f.status, "raw");
        assert_eq!(f.body, "do the thing");
        assert_eq!(f.projects.as_deref(), Some(&["sipag".to_string()][..]));
        let path = Feature::path(dir.path(), project_name(), &f.id);
        assert!(path.exists());
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.starts_with("---\n"));
        assert!(raw.contains("status: raw\n"));
        assert!(raw.contains("projects: [sipag]\n"));
        assert!(raw.ends_with("do the thing"));
    }

    #[test]
    fn round_trip_via_get() {
        let dir = TempDir::new().unwrap();
        let f = Feature::add(
            dir.path(),
            project_name(),
            "the body\nmultiline",
            Some(vec!["a".into(), "b".into()]),
        )
        .unwrap();
        let loaded = Feature::get(dir.path(), project_name(), &f.id)
            .unwrap()
            .unwrap();
        assert_eq!(loaded.id, f.id);
        assert_eq!(loaded.status, "raw");
        assert_eq!(loaded.body, "the body\nmultiline");
        assert_eq!(
            loaded.projects,
            Some(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(loaded.created, f.created);
    }

    #[test]
    fn get_returns_none_for_missing() {
        let dir = TempDir::new().unwrap();
        let res = Feature::get(dir.path(), project_name(), "f-does-not-exist").unwrap();
        assert!(res.is_none());
    }

    #[test]
    fn list_filters_by_status() {
        let dir = TempDir::new().unwrap();
        let raw = Feature::add(dir.path(), project_name(), "raw one", None).unwrap();
        let other = Feature::add(dir.path(), project_name(), "to refine", None).unwrap();
        Feature::update(dir.path(), project_name(), &other.id, |f| {
            f.status = "refined".to_string();
        })
        .unwrap();

        let all = Feature::list(dir.path(), project_name(), None).unwrap();
        assert_eq!(all.len(), 2);

        let only_raw = Feature::list(dir.path(), project_name(), Some("raw")).unwrap();
        assert_eq!(only_raw.len(), 1);
        assert_eq!(only_raw[0].id, raw.id);

        let only_refined = Feature::list(dir.path(), project_name(), Some("refined")).unwrap();
        assert_eq!(only_refined.len(), 1);
        assert_eq!(only_refined[0].id, other.id);
    }

    #[test]
    fn add_log_appends_formatted_line() {
        let dir = TempDir::new().unwrap();
        let f = Feature::add(dir.path(), project_name(), "first", None).unwrap();
        Feature::add_log(dir.path(), project_name(), &f.id, "started").unwrap();
        let loaded = Feature::get(dir.path(), project_name(), &f.id)
            .unwrap()
            .unwrap();
        let lines: Vec<&str> = loaded.body.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "first");
        // Format: "- HH:MM:SS started"
        let log_line = lines[1];
        assert!(
            log_line.starts_with("- "),
            "expected log prefix, got {log_line}"
        );
        assert!(log_line.ends_with(" started"));
        // 2 (prefix) + 8 (HH:MM:SS) + 1 (space) + 7 ("started") = 18
        assert_eq!(log_line.len(), 18);
    }

    #[test]
    fn add_log_on_empty_body_starts_at_log_line() {
        let dir = TempDir::new().unwrap();
        let f = Feature::add(dir.path(), project_name(), "", None).unwrap();
        Feature::add_log(dir.path(), project_name(), &f.id, "kickoff").unwrap();
        let loaded = Feature::get(dir.path(), project_name(), &f.id)
            .unwrap()
            .unwrap();
        assert!(loaded.body.starts_with("- "));
        assert!(loaded.body.ends_with(" kickoff"));
        assert_eq!(loaded.body.lines().count(), 1);
    }

    #[test]
    fn delete_returns_false_on_missing_true_on_success() {
        let dir = TempDir::new().unwrap();
        assert!(!Feature::delete(dir.path(), project_name(), "f-nope").unwrap());
        let f = Feature::add(dir.path(), project_name(), "x", None).unwrap();
        assert!(Feature::delete(dir.path(), project_name(), &f.id).unwrap());
        assert!(!Feature::path(dir.path(), project_name(), &f.id).exists());
    }

    #[test]
    fn update_is_noop_when_feature_missing() {
        let dir = TempDir::new().unwrap();
        let res = Feature::update(dir.path(), project_name(), "f-missing", |f| {
            f.status = "refined".to_string();
        })
        .unwrap();
        assert!(res.is_none());
    }

    #[test]
    fn update_refreshes_updated_timestamp() {
        let dir = TempDir::new().unwrap();
        let f = Feature::add(dir.path(), project_name(), "x", None).unwrap();
        let original_updated = f.updated.clone();
        // Sleep one ms to ensure the millisecond timestamp differs.
        std::thread::sleep(std::time::Duration::from_millis(2));
        let updated = Feature::update(dir.path(), project_name(), &f.id, |f| {
            f.status = "grouped".to_string();
        })
        .unwrap()
        .unwrap();
        assert_eq!(updated.status, "grouped");
        assert_ne!(updated.updated, original_updated);
    }

    #[test]
    fn atomic_write_lands_expected_bytes() {
        let dir = TempDir::new().unwrap();
        let f = Feature::add(
            dir.path(),
            project_name(),
            "the body",
            Some(vec!["sipag".into()]),
        )
        .unwrap();
        let path = Feature::path(dir.path(), project_name(), &f.id);
        let raw = std::fs::read(&path).unwrap();
        // Spot check: first 4 bytes are "---\n".
        assert_eq!(&raw[..4], b"---\n");
        // The body is the tail of the file.
        let s = String::from_utf8(raw).unwrap();
        let (meta, body) = parse_frontmatter(&s).unwrap();
        assert_eq!(body, "the body");
        assert_eq!(
            meta.get("projects"),
            Some(&FrontValue::Array(vec!["sipag".to_string()]))
        );
    }

    #[test]
    fn get_active_by_project_filters_by_membership() {
        let dir = TempDir::new().unwrap();
        // All features live in the "sipag" project's features dir, but only
        // some of them list "sipag" in their projects metadata.
        let f1 = Feature::add(
            dir.path(),
            "sipag",
            "active for sipag",
            Some(vec!["sipag".into(), "katulong".into()]),
        )
        .unwrap();
        Feature::update(dir.path(), "sipag", &f1.id, |f| {
            f.status = "active".to_string();
        })
        .unwrap();

        let f2 = Feature::add(
            dir.path(),
            "sipag",
            "active for other",
            Some(vec!["other".into()]),
        )
        .unwrap();
        Feature::update(dir.path(), "sipag", &f2.id, |f| {
            f.status = "active".to_string();
        })
        .unwrap();

        // A non-active feature is filtered out by status.
        let _f3 = Feature::add(
            dir.path(),
            "sipag",
            "raw for sipag",
            Some(vec!["sipag".into()]),
        )
        .unwrap();

        let res = Feature::get_active_by_project(dir.path(), "sipag").unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].id, f1.id);
    }

    #[test]
    fn refined_field_round_trips_as_json() {
        let dir = TempDir::new().unwrap();
        let f = Feature::add(dir.path(), project_name(), "raw idea", None).unwrap();
        let payload = serde_json::json!({"title": "T", "spec": "S", "subtasks": ["a", "b"]});
        Feature::update(dir.path(), project_name(), &f.id, |f| {
            f.status = "refined".into();
            f.refined = Some(payload.clone());
        })
        .unwrap();
        let loaded = Feature::get(dir.path(), project_name(), &f.id)
            .unwrap()
            .unwrap();
        assert_eq!(loaded.status, "refined");
        assert_eq!(loaded.refined, Some(payload));
    }
}
