use anyhow::Result;
use std::path::{Path, PathBuf};

/// Drain signal: a sentinel file at `<sipag_dir>/drain`.
///
/// When present, workers finish in-flight containers but stop picking up new issues.
/// Compatible with the bash protocol: `sipag drain` creates the file,
/// `sipag resume` removes it.
pub struct DrainSignal(PathBuf);

impl DrainSignal {
    /// Create a DrainSignal bound to `<sipag_dir>/drain`.
    pub fn new(sipag_dir: &Path) -> Self {
        Self(sipag_dir.join("drain"))
    }

    /// Returns true if the drain signal file exists.
    pub fn is_set(&self) -> bool {
        self.0.exists()
    }

    /// Creates the drain signal file (same as `sipag drain`).
    pub fn set(&self) -> Result<()> {
        if let Some(parent) = self.0.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.0, "")?;
        Ok(())
    }

    /// Removes the drain signal file (same as `sipag resume`).
    pub fn clear(&self) -> Result<()> {
        if self.0.exists() {
            std::fs::remove_file(&self.0)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn not_set_by_default() {
        let dir = TempDir::new().unwrap();
        let signal = DrainSignal::new(dir.path());
        assert!(!signal.is_set());
    }

    #[test]
    fn set_creates_file() {
        let dir = TempDir::new().unwrap();
        let signal = DrainSignal::new(dir.path());
        signal.set().unwrap();
        assert!(signal.is_set());
        assert!(dir.path().join("drain").exists());
    }

    #[test]
    fn clear_removes_file() {
        let dir = TempDir::new().unwrap();
        let signal = DrainSignal::new(dir.path());
        signal.set().unwrap();
        signal.clear().unwrap();
        assert!(!signal.is_set());
    }

    #[test]
    fn clear_is_idempotent_when_not_set() {
        let dir = TempDir::new().unwrap();
        let signal = DrainSignal::new(dir.path());
        // Should not error if file doesn't exist
        signal.clear().unwrap();
        assert!(!signal.is_set());
    }

    #[test]
    fn set_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let signal = DrainSignal::new(dir.path());
        signal.set().unwrap();
        signal.set().unwrap(); // second call should be fine
        assert!(signal.is_set());
    }
}
