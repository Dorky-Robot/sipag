use anyhow::Result;
use std::path::Path;

/// Create the sipag directory structure (idempotent).
///
/// Creates `workers/`, `logs/`, `events/`, and `lessons/` under `sipag_dir`.
pub fn init_dirs(sipag_dir: &Path) -> Result<()> {
    let mut created = false;

    for subdir in &["workers", "logs", "events", "lessons"] {
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
        assert!(dir.path().join("workers").exists());
        assert!(dir.path().join("logs").exists());
        assert!(dir.path().join("events").exists());
        assert!(dir.path().join("lessons").exists());
    }

    #[test]
    fn test_init_dirs_idempotent() {
        let dir = TempDir::new().unwrap();
        init_dirs(dir.path()).unwrap();
        init_dirs(dir.path()).unwrap();
    }
}
