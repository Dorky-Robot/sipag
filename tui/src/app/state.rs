/// Task types, task-list refresh, and selection management.
use anyhow::Result;
use std::{fs, path::{Path, PathBuf}, process::Command};

use super::App;

// ── Task ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskStatus {
    Queue,
    Running,
    Done,
    Failed,
}

impl TaskStatus {
    pub fn symbol(&self) -> &'static str {
        match self {
            TaskStatus::Queue => "·",
            TaskStatus::Running => "⧖",
            TaskStatus::Done => "✓",
            TaskStatus::Failed => "✗",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub repo: String,
    pub status: TaskStatus,
}

// ── State methods on App ──────────────────────────────────────────────────────

impl App {
    /// Reload task lists from the filesystem and clamp the selection.
    pub fn refresh_tasks(&mut self) -> Result<()> {
        let mut tasks = Vec::new();
        for (status, subdir) in &[
            (TaskStatus::Queue, "queue"),
            (TaskStatus::Running, "running"),
            (TaskStatus::Done, "done"),
            (TaskStatus::Failed, "failed"),
        ] {
            let dir = self.sipag_dir.join(subdir);
            if !dir.exists() {
                continue;
            }
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            let mut paths: Vec<PathBuf> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|ext| ext == "md"))
                .collect();
            paths.sort();
            for path in paths {
                let id = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let (title, repo) = parse_task_brief(&path);
                tasks.push(Task {
                    id,
                    title,
                    repo,
                    status: status.clone(),
                });
            }
        }
        self.tasks = tasks;

        // Clamp selection to valid range.
        let selected = self.task_list_state.selected().unwrap_or(0);
        if self.tasks.is_empty() {
            self.task_list_state.select(None);
        } else if selected >= self.tasks.len() {
            self.task_list_state.select(Some(self.tasks.len() - 1));
        }
        Ok(())
    }

    pub fn select_next(&mut self) {
        let len = self.tasks.len();
        if len == 0 {
            return;
        }
        let next = self
            .task_list_state
            .selected()
            .map(|i| (i + 1).min(len - 1))
            .unwrap_or(0);
        self.task_list_state.select(Some(next));
    }

    pub fn select_prev(&mut self) {
        if self.tasks.is_empty() {
            return;
        }
        let prev = self
            .task_list_state
            .selected()
            .map(|i| i.saturating_sub(1))
            .unwrap_or(0);
        self.task_list_state.select(Some(prev));
    }

    /// Returns `(queue, running, done, failed)` counts.
    pub fn task_counts(&self) -> (usize, usize, usize, usize) {
        let mut q = 0;
        let mut r = 0;
        let mut d = 0;
        let mut f = 0;
        for t in &self.tasks {
            match t.status {
                TaskStatus::Queue => q += 1,
                TaskStatus::Running => r += 1,
                TaskStatus::Done => d += 1,
                TaskStatus::Failed => f += 1,
            }
        }
        (q, r, d, f)
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Read the first-line title and `repo:` field from a task `.md` file.
///
/// Exposed `pub(crate)` so `executor::mod` can get a task title for display.
pub(crate) fn parse_task_brief(path: &Path) -> (String, String) {
    let content = fs::read_to_string(path).unwrap_or_default();
    let mut in_fm = false;
    let mut fm_done = false;
    let mut repo = String::new();
    let mut title = String::new();
    let mut dashes = 0u32;

    for raw in content.lines() {
        let line = raw.trim();
        if !in_fm && !fm_done && line == "---" {
            in_fm = true;
            dashes += 1;
            continue;
        }
        if in_fm && line == "---" {
            in_fm = false;
            fm_done = true;
            dashes += 1;
            continue;
        }
        if in_fm {
            if let Some(v) = line.strip_prefix("repo:") {
                repo = v.trim().to_string();
            }
        } else if fm_done && !line.is_empty() {
            title = line.to_string();
            break;
        }
    }

    if dashes == 0 {
        // No frontmatter: first non-empty line is the title.
        for raw in content.lines() {
            let line = raw.trim();
            if !line.is_empty() {
                title = line.to_string();
                break;
            }
        }
    }

    if title.is_empty() {
        title = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
    }
    (title, repo)
}

pub(crate) fn which_sipag() -> Option<PathBuf> {
    let output = Command::new("which").arg("sipag").output().ok()?;
    if output.status.success() {
        let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !s.is_empty() {
            return Some(PathBuf::from(s));
        }
    }
    None
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_status_symbols() {
        assert_eq!(TaskStatus::Queue.symbol(), "·");
        assert_eq!(TaskStatus::Running.symbol(), "⧖");
        assert_eq!(TaskStatus::Done.symbol(), "✓");
        assert_eq!(TaskStatus::Failed.symbol(), "✗");
    }

    #[test]
    fn parse_task_brief_with_frontmatter() {
        use std::io::Write;
        let dir = std::env::temp_dir();
        let path = dir.join("test-task-brief.md");
        let mut f = fs::File::create(&path).unwrap();
        write!(
            f,
            "---\nrepo: myrepo\npriority: high\n---\nMy task title\n\nBody text here.\n"
        )
        .unwrap();
        let (title, repo) = parse_task_brief(&path);
        assert_eq!(title, "My task title");
        assert_eq!(repo, "myrepo");
        fs::remove_file(path).ok();
    }

    #[test]
    fn parse_task_brief_no_frontmatter() {
        use std::io::Write;
        let dir = std::env::temp_dir();
        let path = dir.join("test-task-brief-nofm.md");
        let mut f = fs::File::create(&path).unwrap();
        write!(f, "Simple task title\n\nBody here.").unwrap();
        let (title, repo) = parse_task_brief(&path);
        assert_eq!(title, "Simple task title");
        assert!(repo.is_empty());
        fs::remove_file(path).ok();
    }
}
