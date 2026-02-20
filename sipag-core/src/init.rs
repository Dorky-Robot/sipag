use anyhow::Result;
use std::path::Path;

/// Create the sipag directory structure (idempotent).
///
/// Creates `queue/`, `running/`, `done/`, and `failed/` under `sipag_dir`.
/// Prints a line for each directory that is created, then a summary.
pub fn init_dirs(sipag_dir: &Path) -> Result<()> {
    let mut created = false;

    for subdir in &["queue", "running", "done", "failed"] {
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
