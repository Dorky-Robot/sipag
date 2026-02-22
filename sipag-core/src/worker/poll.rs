//! Main worker polling loop — Rust replacement for `lib/worker/loop.sh`.
//!
//! Called by `sipag work <repos...>`. Continuously polls GitHub for ready
//! issues, dispatches Docker workers, and handles recovery/finalization.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};
use std::{fs, thread};

use anyhow::Result;

use super::decision::decide_issue_action;
use super::dispatch::{
    dispatch_conflict_fix, dispatch_pr_iteration, dispatch_worker, is_container_running,
};
use super::event_log::EventLog;
use super::github::{
    count_open_issues, count_open_prs, count_open_sipag_prs, find_conflicted_prs,
    find_prs_needing_iteration, list_labeled_issues, reconcile_closed_prs, reconcile_merged_prs,
    reconcile_stale_in_progress,
};
use super::ports::{GitHubGateway, StateStore};
use super::recovery::{recover_and_finalize, RecoveryOutcome};
use super::status::WorkerStatus;
use super::store::FileStateStore;
use crate::auth;
use crate::config::WorkerConfig;

/// Preview which issues would be dispatched without starting any containers.
///
/// Called by `sipag work --dry-run`. Lists approved issues per repo and shows
/// the dispatch plan for each repo.
pub fn run_dry_run(repos: &[String], cfg: &WorkerConfig) -> Result<()> {
    println!("sipag work --dry-run");
    println!("Label:      {}", cfg.work_label);
    if cfg.max_open_prs > 0 {
        println!("Max open PRs: {}", cfg.max_open_prs);
    }
    println!();

    for repo in repos {
        let all_issues = list_labeled_issues(repo, &cfg.work_label).unwrap_or_default();

        if all_issues.is_empty() {
            println!("[{}] No ready issues.", repo);
            println!();
            continue;
        }

        let issue_strs: Vec<String> = all_issues.iter().map(|n| format!("#{n}")).collect();
        println!(
            "[{}] Found {} ready issue(s): {}",
            repo,
            all_issues.len(),
            issue_strs.join(" ")
        );

        let anchor = all_issues[0];
        if all_issues.len() == 1 {
            println!("Would dispatch 1 worker: #{anchor} \u{2192} sipag/issue-{anchor}-<slug>");
        } else {
            println!(
                "Would dispatch 1 worker ({} issues): {} \u{2192} sipag/group-{}-<slug>",
                all_issues.len(),
                issue_strs.join(" "),
                anchor
            );
        }
        println!();
    }

    // Show back-pressure status across all repos.
    if cfg.max_open_prs > 0 {
        for repo in repos {
            if let Some(open) = count_open_sipag_prs(repo) {
                if open >= cfg.max_open_prs {
                    println!(
                        "Back-pressure: {}/{} open sipag PRs in {} \u{2014} dispatch would be PAUSED.",
                        open, cfg.max_open_prs, repo
                    );
                } else {
                    println!(
                        "Back-pressure: {}/{} open sipag PRs in {} \u{2014} dispatch OK.",
                        open, cfg.max_open_prs, repo
                    );
                }
            }
        }
        println!();
    }

    println!("No containers started (dry-run mode).");
    Ok(())
}

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
    println!("Poll interval: {}s", cfg.poll_interval.as_secs());
    println!("Logs: {}/logs/", sipag_dir.display());
    println!(
        "Started: {}",
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
    );
    println!();

    // ── Structured event log ─────────────────────────────────────────────────
    // Writes JSONL to ~/.sipag/logs/worker.log so a parent session can
    // monitor progress via `tail -f ~/.sipag/logs/worker.log`.
    let logs_dir = sipag_dir.join("logs");
    fs::create_dir_all(&logs_dir).ok();
    let event_log = EventLog::open(&logs_dir);

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

    // ── Kick tracker for back-pressure bypass ─────────────────────────────
    // Set to true when a kick signal is received during sleep; consumed
    // (reset to false) at the top of the next cycle to force dispatch once.
    let mut kicked = false;

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

        // ── Force-dispatch flag (kick signal or SIPAG_FORCE_DISPATCH=1) ──────
        // When active, the back-pressure check is skipped for this cycle.
        let force_dispatch = kicked || std::env::var("SIPAG_FORCE_DISPATCH").as_deref() == Ok("1");
        kicked = false; // consumed — next kick will set it again

        let mut found_work = false;
        let mut dispatch_paused_any = false;

        for repo in repos {
            event_log.cycle_start(repo);
            // ── Per-repo: reconcile + dispatch ──────────────────────────────
            let _ = reconcile_merged_prs(repo);
            let _ = reconcile_closed_prs(repo, &cfg.work_label);

            // Reconcile in-progress issues with no running container.
            {
                let repo_slug = repo.replace('/', "--");
                let _ = reconcile_stale_in_progress(
                    repo,
                    &cfg.work_label,
                    is_container_running,
                    |_repo, issue_num| {
                        store
                            .load(&repo_slug, issue_num)
                            .ok()
                            .flatten()
                            .map(|w| (w.container_name, w.status.as_str().to_string()))
                    },
                );
            }

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
                    // Run synchronously (one at a time).
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
            let all_issues = list_labeled_issues(repo, &cfg.work_label).unwrap_or_default();

            let mut new_issues: Vec<u64> = Vec::new();
            for issue_num in &all_issues {
                let repo_slug = repo.replace('/', "--");
                let worker_status = store
                    .load(&repo_slug, *issue_num)
                    .ok()
                    .flatten()
                    .map(|w| w.status);

                // Check for a single-issue worker branch (fast path).
                let pr_by_branch = gh_gateway
                    .find_pr_for_branch(repo, &format!("sipag/issue-{issue_num}-*"))
                    .ok()
                    .flatten();

                // If no branch PR found, search by "Closes #N" — this catches
                // grouped worker PRs (sipag/group-*) and stale single-issue PRs
                // that the branch glob may have missed.
                let pr_by_body = if pr_by_branch.is_none() {
                    super::github::find_open_pr_for_issue(repo, *issue_num)
                } else {
                    None
                };

                // Also skip issues already marked needs-review (PR exists but
                // body search may not match due to manual edits or draft state).
                let has_needs_review = if pr_by_branch.is_none() && pr_by_body.is_none() {
                    super::github::issue_has_label(repo, *issue_num, "needs-review")
                } else {
                    false
                };

                let has_existing_pr =
                    pr_by_branch.is_some() || pr_by_body.is_some() || has_needs_review;

                match decide_issue_action(worker_status, has_existing_pr) {
                    super::decision::IssueAction::Skip(reason) => {
                        use super::decision::SkipReason;
                        if reason == SkipReason::ExistingPr {
                            // Log which PR is blocking re-dispatch.
                            if let Some(pr) = pr_by_branch.as_ref().or(pr_by_body.as_ref()) {
                                println!(
                                    "[#{}] Skipping — already has PR #{}",
                                    issue_num, pr.number
                                );
                                event_log.issue_skipped(
                                    repo,
                                    *issue_num,
                                    "existing_pr",
                                    Some(pr.number),
                                );
                            } else if has_needs_review {
                                println!("[#{}] Skipping — has needs-review label", issue_num);
                                event_log.issue_skipped(
                                    repo,
                                    *issue_num,
                                    "needs_review_label",
                                    None,
                                );
                            }
                            // Record as done so we skip next cycle.
                            let state = super::state::WorkerState {
                                repo: repo.clone(),
                                issue_num: *issue_num,
                                issue_title: String::new(),
                                branch: String::new(),
                                container_name: String::new(),
                                pr_num: pr_by_branch
                                    .as_ref()
                                    .or(pr_by_body.as_ref())
                                    .map(|p| p.number),
                                pr_url: pr_by_branch
                                    .as_ref()
                                    .or(pr_by_body.as_ref())
                                    .map(|p| p.url.clone()),
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
                event_log.cycle_end(repo);
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
                for &pr_num in &to_iterate {
                    prs_iterating.insert(pr_num);
                    println!("--- PR iteration: #{pr_num} ---");
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
                    println!("--- PR iteration complete ---");
                    println!();
                }
            }

            // New issue workers — gated by back-pressure.
            if !new_issues.is_empty() {
                // ── Back-pressure check ──────────────────────────────────────────
                // When max_open_prs > 0 (and not force-dispatching), count open
                // sipag/* PRs and pause new-issue dispatch if at or above threshold.
                // PR iteration and conflict-fix workers (above) are unaffected.
                let dispatch_paused = if cfg.max_open_prs == 0 || force_dispatch {
                    false
                } else {
                    count_open_sipag_prs(repo)
                        .map(|open| {
                            if open >= cfg.max_open_prs {
                                println!(
                                    "[{}] [{}] {} open PRs (threshold: {}). Pausing dispatch \u{2014} review and merge PRs first.",
                                    hms(),
                                    repo,
                                    open,
                                    cfg.max_open_prs
                                );
                                event_log.back_pressure(repo, open, cfg.max_open_prs);
                                true
                            } else {
                                false
                            }
                        })
                        .unwrap_or(false)
                };

                if dispatch_paused {
                    dispatch_paused_any = true;
                }

                if !dispatch_paused {
                    // All ready issues go to one worker. Claude sees the full
                    // project landscape and crafts the most impactful PR.
                    let issues_to_dispatch = &new_issues[..];
                    let anchor_num = issues_to_dispatch[0];
                    let grouped = issues_to_dispatch.len() > 1;

                    println!(
                        "[{}] {} ready issue(s), dispatching to one worker: {:?}",
                        hms(),
                        issues_to_dispatch.len(),
                        issues_to_dispatch
                    );

                    let container_name = if grouped {
                        format!("sipag-group-{anchor_num}")
                    } else {
                        format!("sipag-issue-{anchor_num}")
                    };
                    event_log.issue_dispatch(repo, issues_to_dispatch, &container_name, grouped);
                    println!("--- Worker: {:?} ---", issues_to_dispatch);

                    let dispatch_start = Instant::now();
                    let result = dispatch_worker(
                        repo,
                        issues_to_dispatch,
                        &cfg,
                        sipag_dir,
                        gh_token.as_deref(),
                        oauth_token.as_deref(),
                        api_key.as_deref(),
                    );
                    let duration_s = dispatch_start.elapsed().as_secs();
                    let success = result.is_ok();

                    // Look up the PR for the branch.
                    let branch_pattern = if grouped {
                        format!("sipag/group-{anchor_num}-*")
                    } else {
                        format!("sipag/issue-{anchor_num}-*")
                    };
                    let pr =
                        super::github::find_pr_for_branch(repo, &branch_pattern).unwrap_or(None);
                    event_log.worker_result(
                        repo,
                        issues_to_dispatch,
                        success,
                        duration_s,
                        pr.as_ref().map(|p| p.number),
                        pr.as_ref().map(|p| p.url.as_str()),
                    );

                    println!("--- Worker complete ---");
                    println!();
                }
            }

            event_log.cycle_end(repo);
            println!("[{}] [{}] Cycle done.", hms(), repo);
        }

        if cfg.once {
            if !found_work {
                println!("[{}] --once: no work found \u{2014} exiting.", hms());
            } else if dispatch_paused_any {
                println!(
                    "[{}] --once: issues found but dispatch paused (back-pressure). Review and merge open PRs, then re-run.",
                    hms()
                );
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
        let chunk = Duration::from_secs(2);
        let mut slept = Duration::ZERO;
        while slept < cfg.poll_interval {
            if sipag_dir.join("kick").exists() {
                println!("[{}] Kick received — polling now.", hms());
                let _ = fs::remove_file(sipag_dir.join("kick"));
                kicked = true; // force dispatch on the next cycle, bypassing back-pressure
                break;
            }
            thread::sleep(chunk);
            slept += chunk;
        }
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
