//! Lifecycle hook runner.
//!
//! Hooks are executable scripts in `${SIPAG_DIR}/hooks/<name>` that fire at
//! worker lifecycle events (on-worker-started, on-worker-completed, etc.).
//! They run asynchronously and never block the caller.
//!
//! Mirrors `sipag_run_hook` from github.sh.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Runs lifecycle hooks from the sipag hooks directory.
pub struct HookRunner {
    hooks_dir: PathBuf,
}

impl HookRunner {
    /// Create a new HookRunner pointing to `${sipag_dir}/hooks/`.
    pub fn new(sipag_dir: &Path) -> Self {
        Self {
            hooks_dir: sipag_dir.join("hooks"),
        }
    }

    /// Run the named hook asynchronously if it exists and is executable.
    ///
    /// The hook script fires in the background — the caller is never blocked.
    /// Environment variables for the hook must be set by the caller before
    /// invoking this method.
    ///
    /// Silently does nothing if the hook doesn't exist or is not executable.
    pub fn run(&self, hook_name: &str) {
        let hook_path = self.hooks_dir.join(hook_name);

        if !hook_path.exists() {
            return;
        }

        if !is_executable(&hook_path) {
            return;
        }

        // Fire and forget — mirrors `"$hook_path" &` in bash
        let _ = Command::new(&hook_path).spawn();
    }
}

/// Check if the file at the given path has the executable bit set (Unix only).
/// On non-Unix platforms, returns true if the file exists.
fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.exists()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn run_missing_hook_does_nothing() {
        let dir = TempDir::new().unwrap();
        let runner = HookRunner::new(dir.path());
        // Should not panic
        runner.run("on-worker-started");
    }

    #[test]
    fn run_non_executable_hook_does_nothing() {
        let dir = TempDir::new().unwrap();
        let hooks_dir = dir.path().join("hooks");
        fs::create_dir(&hooks_dir).unwrap();

        let hook_path = hooks_dir.join("on-worker-started");
        fs::write(&hook_path, "#!/bin/sh\necho hello").unwrap();

        // Remove executable bit
        #[cfg(unix)]
        {
            let mut perms = fs::metadata(&hook_path).unwrap().permissions();
            perms.set_mode(0o644);
            fs::set_permissions(&hook_path, perms).unwrap();
        }

        let runner = HookRunner::new(dir.path());
        // Should not panic or execute
        runner.run("on-worker-started");
    }

    #[test]
    #[cfg(unix)]
    fn run_executable_hook_spawns() {
        let dir = TempDir::new().unwrap();
        let hooks_dir = dir.path().join("hooks");
        fs::create_dir(&hooks_dir).unwrap();

        // Write a hook that creates a sentinel file
        let sentinel = dir.path().join("ran");
        let script = format!("#!/bin/sh\ntouch {}", sentinel.display());
        let hook_path = hooks_dir.join("test-hook");
        fs::write(&hook_path, &script).unwrap();

        let mut perms = fs::metadata(&hook_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&hook_path, perms).unwrap();

        let runner = HookRunner::new(dir.path());
        runner.run("test-hook");

        // Give the async process a moment to run
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(
            sentinel.exists(),
            "hook should have created the sentinel file"
        );
    }

    #[test]
    fn hooks_dir_path_is_sipag_dir_joined_hooks() {
        let dir = TempDir::new().unwrap();
        let runner = HookRunner::new(dir.path());
        assert_eq!(runner.hooks_dir, dir.path().join("hooks"));
    }
}
