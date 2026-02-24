use anyhow::{bail, Context, Result};
use sipag_core::{
    auth,
    config::{default_sipag_dir, Credentials, WorkerConfig},
    docker, init,
    repo::{self, ResolvedRepo},
    worker::github,
};
use std::path::PathBuf;

use crate::orchestrator::{self, find_resumable_session, OrchestratorContext, SessionState};

/// Run a sipag work session using the Rust orchestrator.
///
/// Resolves each directory to a GitHub repo, runs preflight checks, then
/// launches the orchestrator state machine which drives the full autonomous
/// cycle: prune stale issues, analyze diseases, recover in-flight work,
/// and enter the event loop for continuous operation.
pub fn run_work(dirs: &[PathBuf], _resume: Option<&str>) -> Result<()> {
    let dirs = if dirs.is_empty() {
        vec![std::env::current_dir().context("failed to get current directory")?]
    } else {
        dirs.to_vec()
    };

    // Resolve each directory to a GitHub repo.
    let mut repos = Vec::new();
    for dir in &dirs {
        let resolved = repo::resolve_repo(dir)
            .with_context(|| format!("failed to resolve repo for {}", dir.display()))?;
        eprintln!("  {} → {}", dir.display(), resolved.full_name);
        repos.push(resolved);
    }

    // Preflight checks.
    let sipag_dir = default_sipag_dir();
    init::init_dirs(&sipag_dir)?;
    let cfg = WorkerConfig::load(&sipag_dir)?;
    let creds = Credentials::load(&sipag_dir)?;
    auth::preflight_auth(&sipag_dir)?;
    github::preflight_gh_auth()?;
    docker::preflight_docker_running()?;
    docker::preflight_docker_image(&cfg.image)?;

    // Acquire exclusive session locks per repo.
    let _locks = acquire_repo_locks(&sipag_dir, &repos)?;

    // Ensure the sipag label exists on each repo.
    for repo in &repos {
        github::ensure_sipag_label(&repo.full_name);
    }

    // Load or create session.
    let session = match find_resumable_session(&sipag_dir) {
        Some(path) => {
            eprintln!("sipag: resuming session from {}", path.display());
            SessionState::load(&path)?
        }
        None => SessionState::new(&repos),
    };

    let ctx = OrchestratorContext {
        sipag_dir,
        cfg,
        creds,
        repos,
    };

    eprintln!("sipag: launching orchestrator\n");
    orchestrator::run(session, ctx)
}

/// Acquire exclusive file locks for each repo to prevent concurrent sessions.
///
/// Returns the lock file handles — the OS releases the locks when the process exits.
fn acquire_repo_locks(
    sipag_dir: &std::path::Path,
    repos: &[ResolvedRepo],
) -> Result<Vec<std::fs::File>> {
    use std::os::unix::io::AsRawFd;

    let locks_dir = sipag_dir.join("locks");
    std::fs::create_dir_all(&locks_dir)?;

    let mut locks = Vec::new();
    for repo in repos {
        let slug = repo.full_name.replace('/', "--");
        let lock_path = locks_dir.join(format!("{slug}.lock"));

        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("failed to open lock file: {}", lock_path.display()))?;

        // Non-blocking exclusive lock.
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if ret != 0 {
            bail!(
                "Another sipag session is already running for {}. \
                 Kill it first or wait for it to finish.",
                repo.full_name
            );
        }

        // Write PID for diagnostics.
        use std::io::Write;
        let mut f = &file;
        let _ = f.write_all(format!("{}", std::process::id()).as_bytes());

        locks.push(file);
    }

    Ok(locks)
}
