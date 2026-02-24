use anyhow::Result;
use sipag_core::worker::{github, lifecycle};

use super::phase::SessionState;
use super::OrchestratorContext;

/// Recover in-flight work for a single repo.
///
/// Lists open sipag PRs, cross-references with active workers from `sipag ps`,
/// and re-dispatches orphaned PRs. Prioritizes by progress: self-reviewed PRs
/// first, then PRs with real commits, then placeholder-only PRs.
pub fn run_recover(
    repo_index: usize,
    _session: &mut SessionState,
    ctx: &OrchestratorContext,
) -> Result<()> {
    let repo = &ctx.repos[repo_index];
    eprintln!("sipag: recovering in-flight work for {}", repo.full_name);

    // List open sipag PRs.
    let open_prs = github::fetch_open_prs(&repo.full_name).unwrap_or_default();
    let sipag_prs: Vec<_> = open_prs
        .iter()
        .filter(|pr| pr.labels.iter().any(|l| l == "sipag"))
        .collect();

    if sipag_prs.is_empty() {
        eprintln!("sipag: no open sipag PRs for {}", repo.full_name);
        return Ok(());
    }

    // Cross-reference with active workers.
    let workers = lifecycle::scan_workers(&ctx.sipag_dir);
    let active_pr_nums: Vec<u64> = workers
        .iter()
        .filter(|w| !w.phase.is_terminal() && w.repo == repo.full_name)
        .map(|w| w.pr_num)
        .collect();

    for pr in &sipag_prs {
        if active_pr_nums.contains(&pr.number) {
            eprintln!(
                "sipag: PR #{} already has active worker, skipping",
                pr.number
            );
            continue;
        }

        eprintln!(
            "sipag: orphaned PR #{} ({}) — will redispatch on next poll",
            pr.number, pr.title
        );

        // TODO Phase 4: Re-dispatch orphaned PRs
        // sipag_core::worker::dispatch::dispatch_worker(...)
    }

    Ok(())
}
