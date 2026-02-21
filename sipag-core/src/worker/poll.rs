//! Main worker polling loop — Rust replacement for `lib/worker/loop.sh`.
//!
//! Called by `sipag work <repos...>`. Continuously polls GitHub for ready
//! issues, dispatches Docker workers, and handles recovery/finalization.

use std::collections::HashSet;
use std::path::Path;
use std::thread;

use anyhow::Result;

use super::decision::decide_issue_action;
use super::dispatch::{
    dispatch_conflict_fix, dispatch_issue_worker, dispatch_pr_iteration, is_container_running,
};
use super::github::{
    auto_merge_prs, count_open_issues, count_open_prs, find_conflicted_prs,
    find_prs_needing_iteration, has_pr_for_issue, list_approved_issues, reconcile_merged_prs,
};
use super::ports::{GitHubGateway, StateStore};
use super::recovery::{recover_and_finalize, RecoveryOutcome};
use super::status::WorkerStatus;
use super::store::FileStateStore;
use crate::auth;
use crate::config::WorkerConfig;

/// Entry point for `sipag work`.
///
/// Runs the polling loop until a drain signal is detected or `cfg.once` is
/// true and one cycle has completed.
pub fn run_worker_loop(repos: &[String], sipag_dir: &Path, cfg: WorkerConfig) -> Result<()> {
    // ── Print startup banner ─────────────────────────────────────────────────
    println!("sipag work");
    match repos.len() {
        1 => println!("Repo: {}", repos[0]),
        n => println!("Repos ({}): {}", n, repos.join(", ")),
    }
    println!("Label: {}", cfg.work_label);
    println!("Batch size: {}", cfg.batch_size);
    println!("Poll interval: {}s", cfg.poll_interval.as_secs());
    println!("Logs: {}/logs/", sipag_dir.display());
    println!(
        "Started: {}",
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
    );
    println!();

    // ── Resolve credentials once ─────────────────────────────────────────────
    let oauth_token = auth::resolve_token(sipag_dir);
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|k| !k.is_empty());
    let gh_token = super::github::get_gh_token();

    // ── Startup recovery ─────────────────────────────────────────────────────
    // Recover containers that were left running when a previous worker crashed.
    let store = FileStateStore::new(sipag_dir);
    let docker_runtime = DockerRuntime;
    let gh_gateway = super::github::GhGateway::new();
    let outcomes = recover_and_finalize(&docker_runtime, &gh_gateway, &store, &cfg.work_label)?;
    if !outcomes.is_empty() {
        println!(
            "[recovery] {} worker(s) processed on startup",
            outcomes.len()
        );
        for outcome in &outcomes {
            if let RecoveryOutcome::StaleHeartbeat { issue_num } = outcome {
                println!("[recovery] WARNING: worker for issue #{issue_num} has a stale heartbeat");
            }
        }
    }

    // ── In-flight tracker (PR iteration / conflict-fix dedup) ────────────────
    // Uses temp in-memory sets that reset on process restart, mirroring the
    // temp-file approach in lib/worker/dedup.sh.
    let mut prs_iterating: HashSet<u64> = HashSet::new();
    let mut prs_fixing_conflict: HashSet<u64> = HashSet::new();

    // ── Session progress counter for event-driven reminders ───────────────
    let mut completed_this_session: u64 = 0;
    const REMINDER_THRESHOLD: u64 = 10;

    loop {
        // ── Drain check ──────────────────────────────────────────────────────
        if sipag_dir.join("drain").exists() {
            println!(
                "[{}] Drain signal detected. Finishing in-flight work, not picking up new issues.",
                hms()
            );
            break;
        }

        // ── Finalize exited containers ───────────────────────────────────────
        // Runs at the top of each cycle so containers adopted by recovery
        // get their state updated without needing background threads.
        if let Ok(cycle_outcomes) =
            recover_and_finalize(&docker_runtime, &gh_gateway, &store, &cfg.work_label)
        {
            for outcome in &cycle_outcomes {
                match outcome {
                    RecoveryOutcome::StaleHeartbeat { issue_num } => {
                        println!(
                            "[{}] WARNING: worker for issue #{issue_num} has a stale heartbeat (no update in 10+ min)",
                            hms()
                        );
                    }
                    RecoveryOutcome::Finalized { .. } => {
                        completed_this_session += 1;
                    }
                    _ => {}
                }
            }
            if completed_this_session >= REMINDER_THRESHOLD {
                println!(
                    "[sipag] {} issues processed this session. Stay the sipag way — see CLAUDE.md.",
                    completed_this_session
                );
                println!(
                    "[sipag] Review PRs: `gh pr diff N`. Merge: `gh pr merge N --squash --delete-branch`."
                );
                completed_this_session = 0;
            }
        }

        let mut found_work = false;

        for repo in repos {
            // ── Per-repo: reconcile + auto-merge + dispatch ──────────────────
            let _ = reconcile_merged_prs(repo);
            let _ = auto_merge_prs(repo);

            // ── Conflict fixes ───────────────────────────────────────────────
            let conflicted = find_conflicted_prs(repo);
            let to_fix: Vec<u64> = conflicted
                .into_iter()
                .filter(|pr| !prs_fixing_conflict.contains(pr) && !prs_iterating.contains(pr))
                .collect();

            if !to_fix.is_empty() {
                println!(
                    "[{}] {} PR(s) with conflicts to fix: {:?}",
                    hms(),
                    to_fix.len(),
                    to_fix
                );
                found_work = true;
                for pr_num in to_fix {
                    prs_fixing_conflict.insert(pr_num);
                    let repo2 = repo.clone();
                    let cfg2 = cfg.clone();
                    let sipag_dir2 = sipag_dir.to_path_buf();
                    let gh2 = gh_token.clone();
                    let oauth2 = oauth_token.clone();
                    let api2 = api_key.clone();
                    // Run synchronously (one at a time) to keep implementation simple.
                    // Parallelism can be added in a follow-up once batch_size > 1 is needed.
                    let _ = dispatch_conflict_fix(
                        &repo2,
                        pr_num,
                        &cfg2,
                        &sipag_dir2,
                        gh2.as_deref(),
                        oauth2.as_deref(),
                        api2.as_deref(),
                    );
                    prs_fixing_conflict.remove(&pr_num);
                }
            }

            // ── Approved issues ──────────────────────────────────────────────
            let all_issues = list_approved_issues(repo, &cfg.work_label).unwrap_or_default();

            let mut new_issues: Vec<u64> = Vec::new();
            for issue_num in &all_issues {
                let repo_slug = repo.replace('/', "--");
                let worker_status = store
                    .load(&repo_slug, *issue_num)
                    .ok()
                    .flatten()
                    .map(|w| w.status);

                // Use body-content search rather than branch-name glob, because
                // `gh pr list --head "sipag/issue-N-*"` does not do wildcard
                // matching — it looks for a branch literally named with a `*`.
                let has_existing_pr = has_pr_for_issue(repo, *issue_num);

                match decide_issue_action(worker_status, has_existing_pr) {
                    super::decision::IssueAction::Skip(reason) => {
                        use super::decision::SkipReason;
                        if reason == SkipReason::ExistingPr {
                            // Record as done so we skip next cycle.
                            let state = super::state::WorkerState {
                                repo: repo.clone(),
                                issue_num: *issue_num,
                                issue_title: String::new(),
                                branch: String::new(),
                                container_name: String::new(),
                                pr_num: None,
                                pr_url: None,
                                status: WorkerStatus::Done,
                                started_at: None,
                                ended_at: Some(now_utc()),
                                duration_s: None,
                                exit_code: None,
                                log_path: None,
                                last_heartbeat: None,
                                phase: None,
                            };
                            let _ = store.save(&state);
                        }
                    }
                    super::decision::IssueAction::Dispatch => {
                        new_issues.push(*issue_num);
                    }
                }
            }

            // ── PR iteration ─────────────────────────────────────────────────
            let prs_needing = find_prs_needing_iteration(repo);
            let to_iterate: Vec<u64> = prs_needing
                .into_iter()
                .filter(|pr| !prs_iterating.contains(pr) && !prs_fixing_conflict.contains(pr))
                .collect();

            if new_issues.is_empty() && to_iterate.is_empty() {
                let total = count_open_issues(repo)
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "?".to_string());
                let open_prs = count_open_prs(repo)
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "?".to_string());
                println!(
                    "[{}] [{}] {} ready, {} open total, {} PRs open. No work.",
                    hms(),
                    repo,
                    all_issues.len(),
                    total,
                    open_prs
                );
                continue;
            }

            found_work = true;

            // PR iterations first (fix in-flight work before picking up new).
            if !to_iterate.is_empty() {
                println!(
                    "[{}] {} PR(s) needing iteration: {:?}",
                    hms(),
                    to_iterate.len(),
                    to_iterate
                );
                for batch in to_iterate.chunks(cfg.batch_size) {
                    println!("--- PR iteration batch: {:?} ---", batch);
                    for &pr_num in batch {
                        prs_iterating.insert(pr_num);
                        let _ = dispatch_pr_iteration(
                            repo,
                            pr_num,
                            &cfg,
                            sipag_dir,
                            gh_token.as_deref(),
                            oauth_token.as_deref(),
                            api_key.as_deref(),
                        );
                        prs_iterating.remove(&pr_num);
                    }
                    println!("--- PR iteration batch complete ---");
                    println!();
                }
            }

            // New issue workers.
            if !new_issues.is_empty() {
                println!(
                    "[{}] {} new issue(s): {:?}",
                    hms(),
                    new_issues.len(),
                    new_issues
                );
                for batch in new_issues.chunks(cfg.batch_size) {
                    println!("--- Issue batch: {:?} ---", batch);
                    for &issue_num in batch {
                        let _ = dispatch_issue_worker(
                            repo,
                            issue_num,
                            &cfg,
                            sipag_dir,
                            gh_token.as_deref(),
                            oauth_token.as_deref(),
                            api_key.as_deref(),
                        );
                    }
                    println!("--- Batch complete ---");
                    println!();
                }
            }

            println!("[{}] [{}] Cycle done.", hms(), repo);
        }

        if cfg.once {
            if !found_work {
                println!("[{}] --once: no work found — exiting.", hms());
            } else {
                println!("[{}] --once: cycle complete, exiting.", hms());
            }
            break;
        }

        println!(
            "[{}] Next poll in {}s...",
            hms(),
            cfg.poll_interval.as_secs()
        );
        thread::sleep(cfg.poll_interval);
    }

    Ok(())
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn hms() -> String {
    chrono::Utc::now().format("%H:%M:%S").to_string()
}

fn now_utc() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Adapter: checks Docker container status via `docker ps`.
struct DockerRuntime;

impl super::ports::ContainerRuntime for DockerRuntime {
    fn is_running(&self, container_name: &str) -> anyhow::Result<bool> {
        Ok(is_container_running(container_name))
    }
}
