//! Per-repo lessons file for cross-worker learning.
//!
//! Workers fail. The next worker for the same repo shouldn't repeat the same
//! mistakes. This module maintains an append-only markdown file per repo at
//! `~/.sipag/lessons/{owner}--{repo}.md`. The host appends lessons after
//! analyzing failures; worker containers read them before starting work.

use anyhow::Result;
use std::path::{Path, PathBuf};

/// Default maximum bytes to return from `read_lessons`.
/// 8KB is ~20 lessons — enough context without bloating prompts.
pub const DEFAULT_MAX_BYTES: usize = 8 * 1024;

/// Append a lesson entry to the lessons file for a repo.
///
/// Creates `{sipag_dir}/lessons/` and the file if needed.
/// Returns the path to the lessons file.
pub fn append_lesson(sipag_dir: &Path, repo: &str, lesson: &str) -> Result<PathBuf> {
    let lessons_dir = sipag_dir.join("lessons");
    std::fs::create_dir_all(&lessons_dir)?;

    let repo_slug = repo.replace('/', "--");
    let path = lessons_dir.join(format!("{repo_slug}.md"));

    let mut content = String::new();
    // Add a blank line separator if file already has content.
    if path.exists() {
        let existing = std::fs::read_to_string(&path)?;
        if !existing.is_empty() && !existing.ends_with('\n') {
            content.push('\n');
        }
        content.push('\n');
    }
    content.push_str(lesson);
    if !lesson.ends_with('\n') {
        content.push('\n');
    }

    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    file.write_all(content.as_bytes())?;

    Ok(path)
}

/// Read lessons for a repo, truncating from the front if over `max_bytes`.
///
/// Returns `None` if the file doesn't exist or is empty.
/// When truncated, cuts at the nearest `## ` heading boundary so entries
/// stay intact.
pub fn read_lessons(sipag_dir: &Path, repo: &str, max_bytes: usize) -> Result<Option<String>> {
    let repo_slug = repo.replace('/', "--");
    let path = sipag_dir.join("lessons").join(format!("{repo_slug}.md"));

    if !path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&path)?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    if content.len() <= max_bytes {
        return Ok(Some(trimmed.to_string()));
    }

    // Truncate from the front: find the first `## ` heading within the
    // last `max_bytes` of the file to keep entries intact.
    let start = content.len() - max_bytes;
    let tail = &content[start..];
    let result = if let Some(pos) = tail.find("\n## ") {
        // Skip past the newline to start at the heading.
        tail[pos + 1..].trim().to_string()
    } else {
        tail.trim().to_string()
    };

    Ok(Some(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn append_creates_file() {
        let dir = TempDir::new().unwrap();
        let path = append_lesson(dir.path(), "owner/repo", "## Lesson 1\n\nDon't do X.").unwrap();
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("Don't do X."));
    }

    #[test]
    fn append_is_additive() {
        let dir = TempDir::new().unwrap();
        append_lesson(dir.path(), "owner/repo", "## Lesson 1\n\nFirst.").unwrap();
        let path = append_lesson(dir.path(), "owner/repo", "## Lesson 2\n\nSecond.").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("First."));
        assert!(content.contains("Second."));
    }

    #[test]
    fn read_returns_none_for_missing_file() {
        let dir = TempDir::new().unwrap();
        let result = read_lessons(dir.path(), "owner/repo", DEFAULT_MAX_BYTES).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_returns_none_for_empty_file() {
        let dir = TempDir::new().unwrap();
        let lessons_dir = dir.path().join("lessons");
        std::fs::create_dir_all(&lessons_dir).unwrap();
        std::fs::write(lessons_dir.join("owner--repo.md"), "").unwrap();
        let result = read_lessons(dir.path(), "owner/repo", DEFAULT_MAX_BYTES).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_returns_content() {
        let dir = TempDir::new().unwrap();
        append_lesson(dir.path(), "owner/repo", "## Lesson\n\nContent here.").unwrap();
        let result = read_lessons(dir.path(), "owner/repo", DEFAULT_MAX_BYTES).unwrap();
        assert!(result.unwrap().contains("Content here."));
    }

    #[test]
    fn read_truncates_from_front() {
        let dir = TempDir::new().unwrap();
        // Write enough content to exceed a small cap.
        append_lesson(
            dir.path(),
            "o/r",
            "## Old lesson\n\nOld content that is long.",
        )
        .unwrap();
        append_lesson(dir.path(), "o/r", "## New lesson\n\nNew content here.").unwrap();

        // Use a cap that's smaller than total content but large enough for the last entry.
        let result = read_lessons(dir.path(), "o/r", 40).unwrap().unwrap();
        // Should keep the newest entry intact.
        assert!(result.contains("New lesson"));
    }

    #[test]
    fn repo_slug_replaces_slash() {
        let dir = TempDir::new().unwrap();
        let path = append_lesson(dir.path(), "dorky-robot/sipag", "## Test").unwrap();
        let name = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(name, "dorky-robot--sipag.md");
    }
}
