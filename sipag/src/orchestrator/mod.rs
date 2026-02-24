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
pub use phase::{find_resumable_session, SessionState, WorkPhase};

use anyhow::Result;
use sipag_core::config::{Credentials, WorkerConfig};
use sipag_core::repo::ResolvedRepo;
use std::path::PathBuf;
use std::sync::mpsc;

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

/// Run the orchestrator state machine.
///
/// Executes linear phases (Init → Prune → Analyze → Recover) then enters the
/// event loop which blocks on filesystem and timer events. The session is
/// persisted after every phase transition for crash recovery.
pub fn run(mut session: SessionState, ctx: OrchestratorContext) -> Result<()> {
    // Hold the watcher handle to keep it alive for the duration of the event loop.
    let mut event_rx: Option<mpsc::Receiver<WorkEvent>> = None;
    let mut _watcher: Option<notify::RecommendedWatcher> = None;

    loop {
        session.save(&ctx.sipag_dir)?;

        if matches!(session.phase, WorkPhase::Done) {
            eprintln!("sipag: session complete");
            let _ = std::fs::remove_file(ctx.sipag_dir.join("session.json"));
            return Ok(());
        }

        if matches!(session.phase, WorkPhase::EventLoop) {
            // Start watcher on first entry to the event loop.
            if event_rx.is_none() {
                let (rx, w) = events::start_watcher(
                    &ctx.sipag_dir,
                    ctx.cfg.poll_interval,
                    ctx.cfg.heartbeat_stale_secs,
                )?;
                event_rx = Some(rx);
                _watcher = Some(w);
                eprintln!(
                    "sipag: event loop started (poll every {}s)",
                    ctx.cfg.poll_interval
                );
            }

            // Block until next event.
            let rx = event_rx.as_ref().unwrap();
            match rx.recv() {
                Ok(event) => {
                    let next = dispatch_event(event, &mut session);
                    if let Some(phase) = next {
                        session.transition(phase);
                    }
                }
                Err(_) => {
                    eprintln!("sipag: event channel disconnected, shutting down");
                    return Ok(());
                }
            }
            continue;
        }

        // Run non-EventLoop phase.
        let phase = session.phase.clone();
        eprintln!("sipag: phase {:?}", phase);
        let next = run_phase(&phase, &mut session, &ctx)?;
        session.transition(next);

        // Check retro trigger after returning to EventLoop.
        if matches!(session.phase, WorkPhase::EventLoop)
            && session.workers_completed_since_retro >= RETRO_TRIGGER_COUNT
        {
            session.transition(WorkPhase::Retro);
        }
    }
}

/// Map a work event to the next orchestrator phase.
fn dispatch_event(event: WorkEvent, session: &mut SessionState) -> Option<WorkPhase> {
    match event {
        WorkEvent::WorkerFinished { repo, pr_num } => {
            eprintln!("sipag: worker finished for PR #{pr_num} in {repo}");
            session.workers_completed_since_retro += 1;
            Some(WorkPhase::ReviewPr {
                repo,
                pr_num,
                attempt: 0,
            })
        }
        WorkEvent::WorkerFailed { repo, pr_num } => {
            eprintln!("sipag: worker failed for PR #{pr_num} in {repo}");
            session.workers_completed_since_retro += 1;
            Some(WorkPhase::HandleFailed { repo, pr_num })
        }
        WorkEvent::WorkerStale { repo, pr_num } => {
            eprintln!("sipag: worker stale for PR #{pr_num} in {repo}");
            Some(WorkPhase::HandleStale { repo, pr_num })
        }
        WorkEvent::WorkerStarted { repo, pr_num } => {
            eprintln!("sipag: worker started for PR #{pr_num} in {repo}");
            None // informational, stay in EventLoop
        }
        WorkEvent::GithubPoll => {
            eprintln!("sipag: GitHub poll tick");
            Some(WorkPhase::PollCycle)
        }
        WorkEvent::Shutdown => {
            eprintln!("sipag: shutdown requested");
            Some(WorkPhase::Done)
        }
    }
}

/// Dispatch to the appropriate phase handler.
///
/// Each phase handler returns the next phase to transition to. Linear phases
/// (Init → Prune → Analyze → Recover) chain forward. Sub-phases dispatched
/// from the event loop (ReviewPr, HandleFailed, etc.) return EventLoop.
fn run_phase(
    phase: &WorkPhase,
    session: &mut SessionState,
    ctx: &OrchestratorContext,
) -> Result<WorkPhase> {
    match phase {
        WorkPhase::Init => {
            if ctx.repos.is_empty() {
                return Ok(WorkPhase::EventLoop);
            }
            Ok(WorkPhase::PruneStaleIssues { repo_index: 0 })
        }

        WorkPhase::PruneStaleIssues { repo_index } => {
            let repo_index = *repo_index;
            prune::run_prune(repo_index, session, ctx)?;
            if repo_index + 1 < ctx.repos.len() {
                Ok(WorkPhase::PruneStaleIssues {
                    repo_index: repo_index + 1,
                })
            } else {
                Ok(WorkPhase::AnalyzeDiseases { repo_index: 0 })
            }
        }

        WorkPhase::AnalyzeDiseases { repo_index } => {
            let repo_index = *repo_index;
            analyze::run_analyze(repo_index, session, ctx)?;
            if repo_index + 1 < ctx.repos.len() {
                Ok(WorkPhase::AnalyzeDiseases {
                    repo_index: repo_index + 1,
                })
            } else {
                Ok(WorkPhase::RecoverInFlight { repo_index: 0 })
            }
        }

        WorkPhase::RecoverInFlight { repo_index } => {
            let repo_index = *repo_index;
            recover::run_recover(repo_index, session, ctx)?;
            if repo_index + 1 < ctx.repos.len() {
                Ok(WorkPhase::RecoverInFlight {
                    repo_index: repo_index + 1,
                })
            } else {
                Ok(WorkPhase::EventLoop)
            }
        }

        WorkPhase::EventLoop => {
            // EventLoop is handled in the main run() function, not here.
            Ok(WorkPhase::EventLoop)
        }

        WorkPhase::ReviewPr {
            repo,
            pr_num,
            attempt,
        } => review::run_review(repo, *pr_num, *attempt, session, ctx),

        WorkPhase::HandleFailed { repo, pr_num } => {
            failure::handle_failed(repo, *pr_num, session, ctx)?;
            Ok(WorkPhase::EventLoop)
        }

        WorkPhase::HandleStale { repo, pr_num } => {
            failure::handle_stale(repo, *pr_num, session, ctx)?;
            Ok(WorkPhase::EventLoop)
        }

        WorkPhase::PollCycle => {
            poll::run_poll(session, ctx)?;
            Ok(WorkPhase::EventLoop)
        }

        WorkPhase::Retro => {
            retro::run_retro(session, ctx)?;
            session.workers_completed_since_retro = 0;
            Ok(WorkPhase::EventLoop)
        }

        WorkPhase::Done => Ok(WorkPhase::Done),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sipag_core::config::WorkerConfig;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn test_session() -> SessionState {
        SessionState {
            phase: WorkPhase::Init,
            repos: Vec::new(),
            diseases: Vec::new(),
            workers_completed_since_retro: 0,
            started: "2026-01-01T00:00:00Z".to_string(),
            last_transition: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    fn test_ctx(dir: &std::path::Path) -> OrchestratorContext {
        OrchestratorContext {
            sipag_dir: dir.to_path_buf(),
            cfg: WorkerConfig::load(dir).unwrap_or_else(|_| {
                // WorkerConfig::load requires env setup; create minimal config.
                WorkerConfig::load(std::path::Path::new("/tmp")).unwrap()
            }),
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

    // ── dispatch_event tests ──────────────────────────────────────────────

    #[test]
    fn dispatch_event_worker_finished_transitions_to_review() {
        let mut session = test_session();
        let event = WorkEvent::WorkerFinished {
            repo: "o/r".to_string(),
            pr_num: 42,
        };

        let result = dispatch_event(event, &mut session);
        assert!(matches!(
            result,
            Some(WorkPhase::ReviewPr {
                pr_num: 42,
                attempt: 0,
                ..
            })
        ));
        assert_eq!(session.workers_completed_since_retro, 1);
    }

    #[test]
    fn dispatch_event_worker_failed_transitions_to_handle_failed() {
        let mut session = test_session();
        let event = WorkEvent::WorkerFailed {
            repo: "o/r".to_string(),
            pr_num: 10,
        };

        let result = dispatch_event(event, &mut session);
        assert!(matches!(
            result,
            Some(WorkPhase::HandleFailed { pr_num: 10, .. })
        ));
        assert_eq!(session.workers_completed_since_retro, 1);
    }

    #[test]
    fn dispatch_event_worker_stale_transitions_to_handle_stale() {
        let mut session = test_session();
        let event = WorkEvent::WorkerStale {
            repo: "o/r".to_string(),
            pr_num: 5,
        };

        let result = dispatch_event(event, &mut session);
        assert!(matches!(
            result,
            Some(WorkPhase::HandleStale { pr_num: 5, .. })
        ));
        // Stale doesn't increment completed count.
        assert_eq!(session.workers_completed_since_retro, 0);
    }

    #[test]
    fn dispatch_event_worker_started_returns_none() {
        let mut session = test_session();
        let event = WorkEvent::WorkerStarted {
            repo: "o/r".to_string(),
            pr_num: 1,
        };

        let result = dispatch_event(event, &mut session);
        assert!(result.is_none());
    }

    #[test]
    fn dispatch_event_github_poll_transitions_to_poll_cycle() {
        let mut session = test_session();
        let result = dispatch_event(WorkEvent::GithubPoll, &mut session);
        assert!(matches!(result, Some(WorkPhase::PollCycle)));
    }

    #[test]
    fn dispatch_event_shutdown_transitions_to_done() {
        let mut session = test_session();
        let result = dispatch_event(WorkEvent::Shutdown, &mut session);
        assert!(matches!(result, Some(WorkPhase::Done)));
    }

    #[test]
    fn dispatch_event_finished_increments_retro_counter() {
        let mut session = test_session();
        session.workers_completed_since_retro = 2;

        dispatch_event(
            WorkEvent::WorkerFinished {
                repo: "o/r".to_string(),
                pr_num: 1,
            },
            &mut session,
        );

        assert_eq!(session.workers_completed_since_retro, 3);
    }

    // ── run_phase tests ───────────────────────────────────────────────────

    #[test]
    fn init_with_empty_repos_goes_to_event_loop() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();
        let mut session = test_session();
        let ctx = test_ctx(dir.path());

        let next = run_phase(&WorkPhase::Init, &mut session, &ctx).unwrap();
        assert!(matches!(next, WorkPhase::EventLoop));
    }

    #[test]
    fn init_with_repos_goes_to_prune() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();
        let mut session = test_session();
        let ctx = test_ctx_with_repos(dir.path(), 2);

        let next = run_phase(&WorkPhase::Init, &mut session, &ctx).unwrap();
        assert!(matches!(
            next,
            WorkPhase::PruneStaleIssues { repo_index: 0 }
        ));
    }

    #[test]
    fn prune_single_repo_goes_to_analyze() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();
        let mut session = test_session();
        let ctx = test_ctx_with_repos(dir.path(), 1);

        let next = run_phase(
            &WorkPhase::PruneStaleIssues { repo_index: 0 },
            &mut session,
            &ctx,
        )
        .unwrap();
        assert!(matches!(next, WorkPhase::AnalyzeDiseases { repo_index: 0 }));
    }

    #[test]
    fn prune_multi_repo_chains_to_next_repo() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();
        let mut session = test_session();
        let ctx = test_ctx_with_repos(dir.path(), 3);

        let next = run_phase(
            &WorkPhase::PruneStaleIssues { repo_index: 0 },
            &mut session,
            &ctx,
        )
        .unwrap();
        assert!(matches!(
            next,
            WorkPhase::PruneStaleIssues { repo_index: 1 }
        ));

        let next = run_phase(
            &WorkPhase::PruneStaleIssues { repo_index: 1 },
            &mut session,
            &ctx,
        )
        .unwrap();
        assert!(matches!(
            next,
            WorkPhase::PruneStaleIssues { repo_index: 2 }
        ));

        let next = run_phase(
            &WorkPhase::PruneStaleIssues { repo_index: 2 },
            &mut session,
            &ctx,
        )
        .unwrap();
        assert!(matches!(next, WorkPhase::AnalyzeDiseases { repo_index: 0 }));
    }

    #[test]
    fn analyze_single_repo_goes_to_recover() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();
        let mut session = test_session();
        let ctx = test_ctx_with_repos(dir.path(), 1);

        let next = run_phase(
            &WorkPhase::AnalyzeDiseases { repo_index: 0 },
            &mut session,
            &ctx,
        )
        .unwrap();
        assert!(matches!(next, WorkPhase::RecoverInFlight { repo_index: 0 }));
    }

    #[test]
    fn recover_single_repo_goes_to_event_loop() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();
        let mut session = test_session();
        let ctx = test_ctx_with_repos(dir.path(), 1);

        let next = run_phase(
            &WorkPhase::RecoverInFlight { repo_index: 0 },
            &mut session,
            &ctx,
        )
        .unwrap();
        assert!(matches!(next, WorkPhase::EventLoop));
    }

    #[test]
    fn full_linear_phase_sequence_single_repo() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();
        let mut session = test_session();
        let ctx = test_ctx_with_repos(dir.path(), 1);

        // Init → PruneStaleIssues(0)
        let p = run_phase(&WorkPhase::Init, &mut session, &ctx).unwrap();
        assert!(matches!(p, WorkPhase::PruneStaleIssues { repo_index: 0 }));

        // PruneStaleIssues(0) → AnalyzeDiseases(0)
        let p = run_phase(&p, &mut session, &ctx).unwrap();
        assert!(matches!(p, WorkPhase::AnalyzeDiseases { repo_index: 0 }));

        // AnalyzeDiseases(0) → RecoverInFlight(0)
        let p = run_phase(&p, &mut session, &ctx).unwrap();
        assert!(matches!(p, WorkPhase::RecoverInFlight { repo_index: 0 }));

        // RecoverInFlight(0) → EventLoop
        let p = run_phase(&p, &mut session, &ctx).unwrap();
        assert!(matches!(p, WorkPhase::EventLoop));
    }

    #[test]
    fn poll_cycle_returns_event_loop() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();
        let mut session = test_session();
        let ctx = test_ctx_with_repos(dir.path(), 1);

        let next = run_phase(&WorkPhase::PollCycle, &mut session, &ctx).unwrap();
        assert!(matches!(next, WorkPhase::EventLoop));
    }

    #[test]
    fn retro_resets_completed_counter() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();
        let mut session = test_session();
        session.workers_completed_since_retro = 5;
        let ctx = test_ctx(dir.path());

        let next = run_phase(&WorkPhase::Retro, &mut session, &ctx).unwrap();
        assert!(matches!(next, WorkPhase::EventLoop));
        assert_eq!(session.workers_completed_since_retro, 0);
    }

    #[test]
    fn done_returns_done() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();
        let mut session = test_session();
        let ctx = test_ctx(dir.path());

        let next = run_phase(&WorkPhase::Done, &mut session, &ctx).unwrap();
        assert!(matches!(next, WorkPhase::Done));
    }

    // ── Session persistence through phases ────────────────────────────────

    #[test]
    fn session_persists_phase_transitions() {
        let dir = TempDir::new().unwrap();
        let mut session = test_session();

        session.transition(WorkPhase::PruneStaleIssues { repo_index: 1 });
        session.save(dir.path()).unwrap();

        let loaded = SessionState::load(&dir.path().join("session.json")).unwrap();
        assert!(matches!(
            loaded.phase,
            WorkPhase::PruneStaleIssues { repo_index: 1 }
        ));
    }

    #[test]
    fn session_persists_review_pr_phase() {
        let dir = TempDir::new().unwrap();
        let mut session = test_session();

        session.transition(WorkPhase::ReviewPr {
            repo: "owner/repo".to_string(),
            pr_num: 42,
            attempt: 1,
        });
        session.save(dir.path()).unwrap();

        let loaded = SessionState::load(&dir.path().join("session.json")).unwrap();
        match loaded.phase {
            WorkPhase::ReviewPr {
                repo,
                pr_num,
                attempt,
            } => {
                assert_eq!(repo, "owner/repo");
                assert_eq!(pr_num, 42);
                assert_eq!(attempt, 1);
            }
            other => panic!("Expected ReviewPr, got {:?}", other),
        }
    }

    #[test]
    fn session_persists_diseases() {
        let dir = TempDir::new().unwrap();
        let mut session = test_session();
        session.diseases.push(phase::DiseaseCluster {
            name: "test disease".to_string(),
            description: "desc".to_string(),
            issues: vec![1, 2],
            affected_files: vec!["a.rs".to_string()],
            fix_approach: "fix it".to_string(),
        });
        session.save(dir.path()).unwrap();

        let loaded = SessionState::load(&dir.path().join("session.json")).unwrap();
        assert_eq!(loaded.diseases.len(), 1);
        assert_eq!(loaded.diseases[0].name, "test disease");
        assert_eq!(loaded.diseases[0].issues, vec![1, 2]);
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

    // ── Retro trigger logic ───────────────────────────────────────────────

    #[test]
    fn retro_triggers_at_threshold() {
        // Simulate the retro trigger logic from run().
        let mut session = test_session();
        session.phase = WorkPhase::EventLoop;
        session.workers_completed_since_retro = RETRO_TRIGGER_COUNT;

        // This is the check from run():
        let should_trigger = matches!(session.phase, WorkPhase::EventLoop)
            && session.workers_completed_since_retro >= RETRO_TRIGGER_COUNT;
        assert!(should_trigger);
    }

    #[test]
    fn retro_does_not_trigger_below_threshold() {
        let mut session = test_session();
        session.phase = WorkPhase::EventLoop;
        session.workers_completed_since_retro = RETRO_TRIGGER_COUNT - 1;

        let should_trigger = matches!(session.phase, WorkPhase::EventLoop)
            && session.workers_completed_since_retro >= RETRO_TRIGGER_COUNT;
        assert!(!should_trigger);
    }
}
