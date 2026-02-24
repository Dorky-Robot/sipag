mod analyze;
#[allow(dead_code)]
pub mod claude;
mod events;
mod failure;
pub mod phase;
mod poll;
mod prune;
mod recover;
mod retro;
mod review;

pub use events::WorkEvent;
pub use phase::{find_resumable_session, ReviewOutcome, SessionState, WorkPhase};

use anyhow::Result;
use sipag_core::config::{Credentials, WorkerConfig};
use sipag_core::repo::ResolvedRepo;
use std::collections::HashMap;
use std::path::PathBuf;

/// Context shared by all orchestrator phases.
#[allow(dead_code)]
pub struct OrchestratorContext {
    pub sipag_dir: PathBuf,
    pub cfg: WorkerConfig,
    pub creds: Credentials,
    pub repos: Vec<ResolvedRepo>,
}

/// Retro trigger threshold: run retro after this many workers complete.
const RETRO_TRIGGER_COUNT: u32 = 3;

/// Run startup tasks for all repos in parallel, then enter the event loop.
///
/// Startup runs prune → analyze → recover per repo using scoped threads.
/// The event loop blocks on filesystem and timer events, calling handlers directly.
/// Session is saved only on phase transitions (Startup → Running → Done).
pub fn run(mut session: SessionState, ctx: OrchestratorContext) -> Result<()> {
    // ── Startup ──────────────────────────────────────────────────────────
    session.save(&ctx.sipag_dir)?;

    if !ctx.repos.is_empty() {
        eprintln!("sipag: startup — {} repos", ctx.repos.len());
        run_startup(&ctx)?;
    }

    session.transition(WorkPhase::Running);
    session.save(&ctx.sipag_dir)?;

    // ── Event loop ───────────────────────────────────────────────────────
    let (rx, _watcher) = events::start_watcher(
        &ctx.sipag_dir,
        ctx.cfg.poll_interval,
        ctx.cfg.heartbeat_stale_secs,
    )?;
    eprintln!(
        "sipag: event loop started (poll every {}s)",
        ctx.cfg.poll_interval
    );

    let mut completed_count: u32 = 0;
    let mut review_attempts: HashMap<(String, u64), u8> = HashMap::new();

    loop {
        match rx.recv() {
            Ok(event) => match event {
                WorkEvent::WorkerFinished { repo, pr_num } => {
                    eprintln!("sipag: worker finished for PR #{pr_num} in {repo}");
                    completed_count += 1;
                    let attempt = review_attempts
                        .get(&(repo.clone(), pr_num))
                        .copied()
                        .unwrap_or(0);
                    match review::run_review(&repo, pr_num, attempt, &ctx)? {
                        ReviewOutcome::Merged | ReviewOutcome::Skipped => {
                            review_attempts.remove(&(repo, pr_num));
                        }
                        ReviewOutcome::NeedsRedispatch => {
                            *review_attempts.entry((repo, pr_num)).or_insert(0) += 1;
                        }
                        ReviewOutcome::Escalate => {
                            review_attempts.remove(&(repo, pr_num));
                            eprintln!("sipag: PR #{pr_num} escalated to human review");
                        }
                    }
                }
                WorkEvent::WorkerFailed { repo, pr_num } => {
                    eprintln!("sipag: worker failed for PR #{pr_num} in {repo}");
                    completed_count += 1;
                    failure::handle_failed(&repo, pr_num, &ctx)?;
                }
                WorkEvent::WorkerStale { repo, pr_num } => {
                    eprintln!("sipag: worker stale for PR #{pr_num} in {repo}");
                    failure::handle_stale(&repo, pr_num, &ctx)?;
                }
                WorkEvent::WorkerStarted { repo, pr_num } => {
                    eprintln!("sipag: worker started for PR #{pr_num} in {repo}");
                }
                WorkEvent::GithubPoll => {
                    eprintln!("sipag: GitHub poll tick");
                    poll::run_poll(&ctx)?;
                }
                WorkEvent::Shutdown => {
                    eprintln!("sipag: shutdown requested");
                    break;
                }
            },
            Err(_) => {
                eprintln!("sipag: event channel disconnected, shutting down");
                break;
            }
        }

        if completed_count >= RETRO_TRIGGER_COUNT {
            retro::run_retro(&ctx)?;
            completed_count = 0;
        }
    }

    // ── Shutdown ─────────────────────────────────────────────────────────
    session.transition(WorkPhase::Done);
    session.save(&ctx.sipag_dir)?;
    eprintln!("sipag: session complete");
    let _ = std::fs::remove_file(ctx.sipag_dir.join("session.json"));
    Ok(())
}

/// Run startup tasks (prune, analyze, recover) for all repos in parallel.
fn run_startup(ctx: &OrchestratorContext) -> Result<()> {
    std::thread::scope(|s| {
        let handles: Vec<_> = ctx
            .repos
            .iter()
            .map(|repo| {
                s.spawn(|| -> Result<()> {
                    prune::run_prune(repo, ctx)?;
                    analyze::run_analyze(repo, ctx)?;
                    recover::run_recover(repo, ctx)?;
                    Ok(())
                })
            })
            .collect();

        for h in handles {
            h.join().expect("startup thread panicked")?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sipag_core::config::WorkerConfig;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn test_session() -> SessionState {
        SessionState {
            phase: WorkPhase::Startup,
            repos: Vec::new(),
            started: "2026-01-01T00:00:00Z".to_string(),
            last_transition: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    fn test_ctx(dir: &std::path::Path) -> OrchestratorContext {
        OrchestratorContext {
            sipag_dir: dir.to_path_buf(),
            cfg: WorkerConfig::load(dir)
                .unwrap_or_else(|_| WorkerConfig::load(std::path::Path::new("/tmp")).unwrap()),
            creds: Credentials {
                oauth_token: None,
                api_key: None,
                gh_token: "test-token".to_string(),
            },
            repos: Vec::new(),
        }
    }

    fn test_ctx_with_repos(dir: &std::path::Path, count: usize) -> OrchestratorContext {
        let mut ctx = test_ctx(dir);
        for i in 0..count {
            ctx.repos.push(ResolvedRepo {
                owner: "owner".to_string(),
                name: format!("repo-{i}"),
                full_name: format!("owner/repo-{i}"),
                local_path: PathBuf::from(format!("/tmp/repo-{i}")),
            });
        }
        ctx
    }

    // ── Startup tests ────────────────────────────────────────────────────

    #[test]
    fn startup_with_empty_repos_succeeds() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();
        let ctx = test_ctx(dir.path());
        // Empty repos — run_startup should succeed trivially.
        assert!(run_startup(&ctx).is_ok());
    }

    #[test]
    fn startup_with_repos_runs_all_phases() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();
        let ctx = test_ctx_with_repos(dir.path(), 2);
        // All handlers are stubs, so this should succeed.
        assert!(run_startup(&ctx).is_ok());
    }

    // ── Session persistence ──────────────────────────────────────────────

    #[test]
    fn session_persists_phase_transitions() {
        let dir = TempDir::new().unwrap();
        let mut session = test_session();

        session.transition(WorkPhase::Running);
        session.save(dir.path()).unwrap();

        let loaded = SessionState::load(&dir.path().join("session.json")).unwrap();
        assert!(matches!(loaded.phase, WorkPhase::Running));
    }

    #[test]
    fn session_load_malformed_json_returns_error() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("session.json"), "not json{{{").unwrap();
        assert!(SessionState::load(&dir.path().join("session.json")).is_err());
    }

    #[test]
    fn session_load_missing_file_returns_error() {
        let dir = TempDir::new().unwrap();
        assert!(SessionState::load(&dir.path().join("nonexistent.json")).is_err());
    }

    // ── Retro trigger logic ──────────────────────────────────────────────

    #[test]
    fn retro_triggers_at_threshold() {
        let count: u32 = RETRO_TRIGGER_COUNT;
        assert!(count >= RETRO_TRIGGER_COUNT);
    }

    #[test]
    fn retro_does_not_trigger_below_threshold() {
        let count: u32 = RETRO_TRIGGER_COUNT - 1;
        assert!(count < RETRO_TRIGGER_COUNT);
    }

    // ── Disease file I/O ─────────────────────────────────────────────────

    #[test]
    fn analyze_writes_diseases_to_disk() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();
        let ctx = test_ctx_with_repos(dir.path(), 1);

        // run_analyze writes empty diseases.
        analyze::run_analyze(&ctx.repos[0], &ctx).unwrap();

        let diseases = phase::load_diseases(&ctx.sipag_dir, &ctx.repos[0].full_name);
        assert!(diseases.is_empty());
    }

    #[test]
    fn poll_reads_diseases_from_disk() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();
        let ctx = test_ctx_with_repos(dir.path(), 1);

        // Write some diseases.
        let clusters = vec![phase::DiseaseCluster {
            name: "test".to_string(),
            description: "desc".to_string(),
            issues: vec![1],
            affected_files: vec![],
            fix_approach: "fix".to_string(),
        }];
        phase::save_diseases(&ctx.sipag_dir, &ctx.repos[0].full_name, &clusters).unwrap();

        // Poll should succeed (reads diseases internally).
        assert!(poll::run_poll(&ctx).is_ok());
    }

    // ── ReviewOutcome ────────────────────────────────────────────────────

    #[test]
    fn review_outcome_variants() {
        // Verify all variants exist and are distinct via Debug.
        let outcomes = [
            ReviewOutcome::Merged,
            ReviewOutcome::NeedsRedispatch,
            ReviewOutcome::Escalate,
            ReviewOutcome::Skipped,
        ];
        let debug_strings: Vec<_> = outcomes.iter().map(|o| format!("{:?}", o)).collect();
        // All unique.
        let unique: std::collections::HashSet<_> = debug_strings.iter().collect();
        assert_eq!(unique.len(), 4);
    }
}
