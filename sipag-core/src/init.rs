use anyhow::Result;
use std::path::Path;

/// Create the sipag directory structure (idempotent).
///
/// Creates the standard subdirectories under `sipag_dir`:
/// - `queue/`, `running/`, `done/`, `failed/` — task queue dirs
/// - `workers/` — JSON state files for `sipag work` issue workers
/// - `logs/`    — persistent worker log files (replaces /tmp)
/// - `seen/`    — per-repo dedup files (one file per OWNER--REPO)
pub fn init_dirs(sipag_dir: &Path) -> Result<()> {
    let mut created = false;

    for subdir in &["queue", "running", "done", "failed", "workers", "logs", "seen"] {
        let path = sipag_dir.join(subdir);
        if !path.exists() {
            std::fs::create_dir_all(&path)?;
            println!("Created: {}", path.display());
            created = true;
        }
    }

    if created {
        println!("Initialized: {}", sipag_dir.display());
    } else {
        println!("Already initialized: {}", sipag_dir.display());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_init_dirs() {
        let dir = TempDir::new().unwrap();
        init_dirs(dir.path()).unwrap();
        assert!(dir.path().join("queue").exists());
        assert!(dir.path().join("running").exists());
        assert!(dir.path().join("done").exists());
        assert!(dir.path().join("failed").exists());
        assert!(dir.path().join("workers").exists());
        assert!(dir.path().join("logs").exists());
        assert!(dir.path().join("seen").exists());
    }

    #[test]
    fn test_init_dirs_idempotent() {
        let dir = TempDir::new().unwrap();
        init_dirs(dir.path()).unwrap();
        // Should not fail on second call
        init_dirs(dir.path()).unwrap();
    }
}
