//! Lifecycle event files for external observation.
//!
//! sipag writes plain-text event files to `~/.sipag/events/` when lifecycle
//! events occur (e.g. worker failures). External systems — tao, email, Slack,
//! whatever — can watch that directory and act on new files.
//!
//! Unix philosophy: sipag writes files, something else reads them.

use anyhow::Result;
use std::path::{Path, PathBuf};

/// Write a lifecycle event file to `{sipag_dir}/events/`.
///
/// Returns the path to the created file. Filenames are timestamped and sorted
/// chronologically: `{ISO8601}-{event_type}-{repo_slug}.md`.
pub fn write_event(
    sipag_dir: &Path,
    event_type: &str,
    repo: &str,
    subject: &str,
    body: &str,
) -> Result<PathBuf> {
    let events_dir = sipag_dir.join("events");
    std::fs::create_dir_all(&events_dir)?;

    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let repo_slug = repo.replace('/', "--");
    let filename = format!("{timestamp}-{event_type}-{repo_slug}.md");
    let path = events_dir.join(&filename);

    let content = format!("Subject: {subject}\n\n{body}\n");
    std::fs::write(&path, &content)?;

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn write_event_creates_file() {
        let dir = TempDir::new().unwrap();
        let path = write_event(
            dir.path(),
            "worker-failed",
            "owner/repo",
            "Worker failed for PR #42 in owner/repo",
            "The worker implementing PR #42 has failed.\n\nError: claude exited with code 1",
        )
        .unwrap();

        assert!(path.exists());
        assert!(path.starts_with(dir.path().join("events")));
    }

    #[test]
    fn write_event_content_is_correct() {
        let dir = TempDir::new().unwrap();
        let path = write_event(
            dir.path(),
            "worker-failed",
            "owner/repo",
            "Worker failed for PR #42",
            "Details here.",
        )
        .unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.starts_with("Subject: Worker failed for PR #42\n\n"));
        assert!(content.contains("Details here."));
        assert!(content.ends_with('\n'));
    }

    #[test]
    fn write_event_filename_is_sortable() {
        let dir = TempDir::new().unwrap();
        let path1 = write_event(dir.path(), "worker-failed", "a/b", "s1", "b1").unwrap();
        // Small delay not needed — filenames include seconds, and both land in the same second.
        // What matters is the format is lexicographically sortable.
        let path2 = write_event(dir.path(), "worker-started", "a/b", "s2", "b2").unwrap();

        let name1 = path1.file_name().unwrap().to_str().unwrap();
        let name2 = path2.file_name().unwrap().to_str().unwrap();

        // Both start with a timestamp prefix (YYYYMMDDTHHMMSSZ).
        assert!(name1.len() > 16);
        assert!(name2.len() > 16);
        // Timestamp portion is identical or ordered.
        assert!(name1[..16] <= name2[..16]);
    }

    #[test]
    fn write_event_repo_slug_replaces_slash() {
        let dir = TempDir::new().unwrap();
        let path = write_event(dir.path(), "worker-failed", "dorky-robot/sipag", "s", "b").unwrap();
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(name.contains("dorky-robot--sipag"));
        assert!(!name.contains('/'));
    }

    #[test]
    fn write_event_creates_events_dir() {
        let dir = TempDir::new().unwrap();
        assert!(!dir.path().join("events").exists());
        write_event(dir.path(), "test", "o/r", "s", "b").unwrap();
        assert!(dir.path().join("events").exists());
    }
}
