use anyhow::Result;
use sipag_core::worker::github;

use super::phase::SessionState;
use super::OrchestratorContext;

/// Run a full poll cycle: orphan redispatch, back-pressure check, issue clustering, dispatch.
///
/// Priority order: finish → recover → start new.
/// 1. Re-dispatch orphaned in-flight PRs (most-progressed first)
/// 2. Check back-pressure (open sipag PRs vs max_open_prs)
/// 3. Pick up new ready issues, cluster by disease, create PRs, dispatch workers
pub fn run_poll(_session: &mut SessionState, ctx: &OrchestratorContext) -> Result<()> {
    eprintln!("sipag: poll cycle starting");

    for (i, repo) in ctx.repos.iter().enumerate() {
        eprintln!("sipag: polling {}", repo.full_name);

        // 1. Check back-pressure.
        let open_sipag_count = github::count_open_sipag_prs(&repo.full_name).unwrap_or(0);
        if ctx.cfg.max_open_prs > 0 && open_sipag_count >= ctx.cfg.max_open_prs {
            eprintln!(
                "sipag: back-pressure for {} ({} open PRs, max {})",
                repo.full_name, open_sipag_count, ctx.cfg.max_open_prs
            );
            continue;
        }

        // 2. Fetch ready issues.
        let ready_issues =
            github::list_labeled_issues(&repo.full_name, &ctx.cfg.work_label).unwrap_or_default();

        if ready_issues.is_empty() {
            eprintln!("sipag: no ready issues for {}", repo.full_name);
            continue;
        }

        eprintln!(
            "sipag: {} ready issues for {}: {:?}",
            ready_issues.len(),
            repo.full_name,
            ready_issues
        );

        // TODO Phase 5: Implement issue clustering and PR creation
        // 1. Read all ready issue bodies
        // 2. Invoke Claude to cluster by disease
        // 3. For each cluster: create branch, create PR, dispatch worker
        // 4. Label transition: ready → in-progress
        let _ = i; // suppress unused warning
    }

    eprintln!("sipag: poll cycle complete");
    Ok(())
}
