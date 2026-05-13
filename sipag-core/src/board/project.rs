//! Project data model — stored at `~/.sipag/projects/{name}/project.toml`.

use anyhow::{Context, Result};
use serde::{Deserialize, Deserializer, Serialize};
use std::path::Path;

use super::atomic_write;

/// What kind of top-level container this project is.
///
/// `Objective` (the default) is the OKR-shaped container: holds key
/// results and outcome-driven tasks. `Standing` is for perpetual
/// upkeep — architecture reviews, dep audits, one-off firefights —
/// work that doesn't ladder up to an outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectKind {
    #[default]
    Objective,
    Standing,
}

/// A status column on the board. Stored as a TOML table in
/// `project.toml`'s `statuses` array, with the bare-string form
/// (`statuses = ["todo", "done"]`) also accepted for backward
/// compatibility — legacy projects load unchanged, then upgrade to the
/// table form on the next save.
///
/// `description` is free-form prose that explains what work in this
/// column actually means — the dispatch gate feeds these descriptions
/// to gemma4 so it can classify a session's current state into one of
/// the project's columns.
///
/// `dispatchable = true` marks the one status whose presence means
/// "ready to fire the agent command." Exactly one per project should
/// set this; the gate refuses to dispatch when zero or multiple are
/// flagged (see [`Project::dispatchable_status`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Status {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub dispatchable: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Status {
    /// Construct a name-only status (no description, not dispatchable).
    /// Used when promoting legacy string statuses and when callers pass
    /// `Option<Vec<String>>` into `create_project_with_kind`.
    pub fn name_only(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            dispatchable: false,
        }
    }
}

// Custom Deserialize so each entry in `statuses` can be either a bare
// string ("todo") or a table ({ name = "todo", description = "...",
// dispatchable = true }). Strings promote to Status::name_only. Tables
// deserialize the full shape with sensible defaults.
impl<'de> Deserialize<'de> for Status {
    fn deserialize<D>(d: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Bare(String),
            Full {
                name: String,
                #[serde(default)]
                description: String,
                #[serde(default)]
                dispatchable: bool,
            },
        }
        Ok(match Repr::deserialize(d)? {
            Repr::Bare(name) => Status::name_only(name),
            Repr::Full {
                name,
                description,
                dispatchable,
            } => Status {
                name,
                description,
                dispatchable,
            },
        })
    }
}

/// A project on the board. In the new objective-shaped model the
/// project is an *initiative* — the current best means of approaching
/// one or more objectives, listed in `serves`. Projects without
/// `serves` are orphan initiatives until linked.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub repo: String,
    #[serde(default)]
    pub kind: ProjectKind,
    #[serde(default = "default_statuses")]
    pub statuses: Vec<Status>,
    /// Objective ids this project serves. Empty = orphan initiative.
    /// A project may serve more than one objective.
    #[serde(default)]
    pub serves: Vec<String>,
}

/// Default kanban columns plus the gate-only `needs-human` status. The
/// descriptions feed the dispatch gate (see `sipag-core/src/gate.rs`
/// once Step 4 lands); each one tells gemma4 what work in that column
/// actually looks like so it can route a session into the right place.
pub(super) fn default_statuses() -> Vec<Status> {
    vec![
        Status {
            name: "backlog".into(),
            description: "Captured idea or rough note that has not been groomed yet — not ready \
                          to be picked up."
                .into(),
            dispatchable: false,
        },
        Status {
            name: "todo".into(),
            description: "Groomed and ready to dispatch. The session is logged in, idle, and \
                          waiting for work."
                .into(),
            dispatchable: true,
        },
        Status {
            name: "in-progress".into(),
            description: "An agent is actively working on this task in its katulong session."
                .into(),
            dispatchable: false,
        },
        Status {
            name: "needs-human".into(),
            description: "Gemma4 saw something it cannot resolve alone (login prompt, \
                          permission request, OAuth flow, rate limit). A human must intervene \
                          before this task can move on."
                .into(),
            dispatchable: false,
        },
        Status {
            name: "review".into(),
            description: "Agent claims the work is done and is waiting for a human to verify \
                          before it moves to `done`."
                .into(),
            dispatchable: false,
        },
        Status {
            name: "done".into(),
            description: "Accepted. No further work needed.".into(),
            dispatchable: false,
        },
    ]
}

impl Project {
    /// Load a project by name.
    pub fn load(sipag_dir: &Path, name: &str) -> Result<Self> {
        let path = sipag_dir.join("projects").join(name).join("project.toml");
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("project '{name}' not found"))?;
        let project: Self = toml::from_str(&content)
            .with_context(|| format!("invalid TOML in {}", path.display()))?;
        Ok(project)
    }

    /// Save this project to disk.
    pub fn save(&self, sipag_dir: &Path) -> Result<()> {
        let project_dir = sipag_dir.join("projects").join(&self.name);
        std::fs::create_dir_all(project_dir.join("tasks"))?;
        std::fs::create_dir_all(project_dir.join("roles"))?;

        let path = project_dir.join("project.toml");
        let content = toml::to_string_pretty(self)?;
        atomic_write(&path, content.as_bytes())
    }

    /// Convenience for callers that only care about column names — the
    /// TUI's kanban view, the HTTP `ProjectView`, and `add_task`'s
    /// default-status pick all want bare strings, not the full
    /// description/dispatchable metadata.
    pub fn status_names(&self) -> Vec<String> {
        self.statuses.iter().map(|s| s.name.clone()).collect()
    }

    /// Return the status flagged `dispatchable = true`. Errors when
    /// zero or multiple statuses claim it — both shapes are config
    /// mistakes the dispatch gate must refuse to fire on. The error
    /// message tells the operator exactly how to fix project.toml.
    pub fn dispatchable_status(&self) -> Result<&Status> {
        let mut hits = self.statuses.iter().filter(|s| s.dispatchable);
        let first = hits.next().with_context(|| {
            format!(
                "project '{}' has no dispatchable status — mark one of its statuses with \
                 `dispatchable = true` in project.toml",
                self.name
            )
        })?;
        if hits.next().is_some() {
            anyhow::bail!(
                "project '{}' has multiple statuses marked dispatchable — only one should be \
                 flagged in project.toml",
                self.name
            );
        }
        Ok(first)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn project_round_trip() {
        let dir = TempDir::new().unwrap();
        let project = Project {
            name: "katulong".to_string(),
            repo: "dorky-robot/katulong".to_string(),
            kind: ProjectKind::Objective,
            statuses: default_statuses(),
            serves: Vec::new(),
        };
        project.save(dir.path()).unwrap();

        let loaded = Project::load(dir.path(), "katulong").unwrap();
        assert_eq!(loaded.name, "katulong");
        assert_eq!(loaded.repo, "dorky-robot/katulong");
        assert_eq!(loaded.statuses.len(), 6);
        assert_eq!(loaded.kind, ProjectKind::Objective);
        // Default statuses must be parseable by dispatchable_status —
        // the gate depends on `todo` being the canonical ready state.
        assert_eq!(loaded.dispatchable_status().unwrap().name, "todo");
    }

    #[test]
    fn project_kind_defaults_to_objective_when_missing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("projects/legacy/project.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // TOML without `kind` — represents projects from before this field existed.
        std::fs::write(
            &path,
            "name = \"legacy\"\nrepo = \"a/b\"\nstatuses = [\"todo\"]\n",
        )
        .unwrap();

        let loaded = Project::load(dir.path(), "legacy").unwrap();
        assert_eq!(loaded.kind, ProjectKind::Objective);
    }

    #[test]
    fn legacy_string_statuses_load_as_name_only() {
        // Pre-Status-table schema: bare string array. Must still load,
        // promoted to Status { name, description: "", dispatchable: false }.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("projects/legacy/project.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "name = \"legacy\"\nrepo = \"a/b\"\nstatuses = [\"backlog\", \"todo\", \"done\"]\n",
        )
        .unwrap();

        let loaded = Project::load(dir.path(), "legacy").unwrap();
        assert_eq!(loaded.statuses.len(), 3);
        assert_eq!(loaded.statuses[1].name, "todo");
        assert!(loaded.statuses[1].description.is_empty());
        assert!(!loaded.statuses[1].dispatchable);
    }

    #[test]
    fn legacy_string_statuses_upgrade_to_table_on_save_round_trip() {
        // The module-level docstring promises legacy `statuses =
        // ["a", "b"]` projects load unchanged, then "upgrade to
        // the table form on the next save." This test pins the
        // load → save → reload round-trip: after a save, the
        // reloaded project must still parse cleanly, and any
        // explicit description / dispatchable set in code between
        // load and save must survive the round-trip.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("projects/legacy/project.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "name = \"legacy\"\nrepo = \"a/b\"\nstatuses = [\"todo\", \"done\"]\n",
        )
        .unwrap();

        // Load → mutate one status → save.
        let mut loaded = Project::load(dir.path(), "legacy").unwrap();
        assert_eq!(loaded.statuses.len(), 2);
        loaded.statuses[0].description = "ready".into();
        loaded.statuses[0].dispatchable = true;
        loaded.save(dir.path()).unwrap();

        // Reload — the saved form must round-trip cleanly and
        // preserve the mutations.
        let reloaded = Project::load(dir.path(), "legacy").unwrap();
        assert_eq!(reloaded.statuses.len(), 2);
        assert_eq!(reloaded.statuses[0].name, "todo");
        assert_eq!(reloaded.statuses[0].description, "ready");
        assert!(reloaded.statuses[0].dispatchable);
        assert_eq!(reloaded.statuses[1].name, "done");

        // Also assert the on-disk shape: after save, the project
        // is in table form (`[[statuses]]`), not the legacy
        // string-array form. Pins the docstring's "upgrade to
        // the table form on the next save" promise on the
        // serialization side, not just the round-trip side.
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(
            on_disk.contains("[[statuses]]"),
            "saved project should be in table form; got:\n{on_disk}"
        );
        assert!(!on_disk.contains("statuses = ["));
    }

    #[test]
    fn new_table_statuses_load_with_full_metadata() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("projects/new/project.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"
name = "new"
repo = "a/b"

[[statuses]]
name = "todo"
description = "ready to ship"
dispatchable = true

[[statuses]]
name = "done"
description = "shipped"
"#,
        )
        .unwrap();

        let loaded = Project::load(dir.path(), "new").unwrap();
        assert_eq!(loaded.statuses.len(), 2);
        assert_eq!(loaded.statuses[0].name, "todo");
        assert_eq!(loaded.statuses[0].description, "ready to ship");
        assert!(loaded.statuses[0].dispatchable);
        assert_eq!(loaded.statuses[1].name, "done");
        assert!(loaded.statuses[1].description == "shipped");
        assert!(!loaded.statuses[1].dispatchable);
    }

    #[test]
    fn mixed_string_and_table_statuses_load() {
        // Allowed for incremental migration — operator can upgrade one
        // status at a time without touching the others.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("projects/mix/project.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"
name = "mix"
repo = "a/b"
statuses = ["backlog", { name = "todo", dispatchable = true }, "done"]
"#,
        )
        .unwrap();

        let loaded = Project::load(dir.path(), "mix").unwrap();
        assert_eq!(loaded.statuses.len(), 3);
        assert_eq!(loaded.statuses[0].name, "backlog");
        assert!(!loaded.statuses[0].dispatchable);
        assert_eq!(loaded.statuses[1].name, "todo");
        assert!(loaded.statuses[1].dispatchable);
        assert_eq!(loaded.statuses[2].name, "done");
    }

    #[test]
    fn dispatchable_status_errors_when_none_flagged() {
        // Legacy projects (string statuses) have zero dispatchable
        // flags — the gate must refuse rather than guess which column
        // means "ready."
        let proj = Project {
            name: "noflag".into(),
            repo: "a/b".into(),
            kind: ProjectKind::Objective,
            statuses: vec![Status::name_only("todo"), Status::name_only("done")],
            serves: vec![],
        };
        let err = proj.dispatchable_status().unwrap_err().to_string();
        assert!(err.contains("no dispatchable status"));
        assert!(err.contains("noflag"));
    }

    #[test]
    fn dispatchable_status_errors_when_multiple_flagged() {
        let proj = Project {
            name: "twoflag".into(),
            repo: "a/b".into(),
            kind: ProjectKind::Objective,
            statuses: vec![
                Status {
                    name: "todo".into(),
                    description: "".into(),
                    dispatchable: true,
                },
                Status {
                    name: "ready".into(),
                    description: "".into(),
                    dispatchable: true,
                },
            ],
            serves: vec![],
        };
        let err = proj.dispatchable_status().unwrap_err().to_string();
        assert!(err.contains("multiple statuses marked dispatchable"));
    }

    #[test]
    fn status_names_returns_bare_strings() {
        let proj = Project {
            name: "p".into(),
            repo: "a/b".into(),
            kind: ProjectKind::Objective,
            statuses: vec![
                Status::name_only("todo"),
                Status {
                    name: "doing".into(),
                    description: "in progress".into(),
                    dispatchable: false,
                },
                Status::name_only("done"),
            ],
            serves: vec![],
        };
        assert_eq!(proj.status_names(), vec!["todo", "doing", "done"]);
    }

    #[test]
    fn status_serialize_skips_empty_fields() {
        // Round-trip a name-only Status — the table should serialize
        // without empty `description` or false `dispatchable` keys, so
        // TOML files don't grow noisy diffs after a save.
        let s = Status::name_only("todo");
        let toml = toml::to_string(&s).unwrap();
        assert!(toml.contains("name = \"todo\""));
        assert!(!toml.contains("description"));
        assert!(!toml.contains("dispatchable"));
    }

    #[test]
    fn project_save_creates_directories() {
        let dir = TempDir::new().unwrap();
        let project = Project {
            name: "newproj".to_string(),
            repo: "a/b".to_string(),
            kind: ProjectKind::Standing,
            statuses: vec![Status::name_only("open"), Status::name_only("closed")],
            serves: Vec::new(),
        };
        project.save(dir.path()).unwrap();

        assert!(dir.path().join("projects/newproj/tasks").is_dir());
        assert!(dir.path().join("projects/newproj/roles").is_dir());
        assert!(dir.path().join("projects/newproj/project.toml").exists());
    }

    #[test]
    fn project_load_nonexistent_fails() {
        let dir = TempDir::new().unwrap();
        assert!(Project::load(dir.path(), "nope").is_err());
    }
}
