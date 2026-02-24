use anyhow::Result;
use sipag_core::worker::github;

use super::OrchestratorContext;

/// Run a full poll cycle: orphan redispatch, back-pressure check, issue clustering, dispatch.
///
/// Priority order: finish → recover → start new.
/// 1. Re-dispatch orphaned in-flight PRs (most-progressed first)
/// 2. Check back-pressure (open sipag PRs vs max_open_prs)
/// 3. Pick up new ready issues, cluster by disease, create PRs, dispatch workers
pub fn run_poll(ctx: &OrchestratorContext) -> Result<()> {
    eprintln!("sipag: poll cycle starting");

    for repo in &ctx.repos {
        eprintln!("sipag: polling {}", repo.full_name);

        // 1. Check back-pressure.
        let open_sipag_count = match github::count_open_sipag_prs(&repo.full_name) {
            Ok(count) => count,
            Err(e) => {
                eprintln!(
                    "sipag: failed to count open PRs for {}, skipping (fail closed): {e:#}",
                    repo.full_name
                );
                continue;
            }
        };
        if ctx.cfg.max_open_prs > 0 && open_sipag_count >= ctx.cfg.max_open_prs {
            eprintln!(
                "sipag: back-pressure for {} ({} open PRs, max {})",
                repo.full_name, open_sipag_count, ctx.cfg.max_open_prs
            );
            continue;
        }

        // 2. Fetch ready issues.
        let ready_issues = match github::list_labeled_issues(&repo.full_name, &ctx.cfg.work_label) {
            Ok(issues) => issues,
            Err(e) => {
                eprintln!(
                    "sipag: failed to list issues for {}, skipping (fail closed): {e:#}",
                    repo.full_name
                );
                continue;
            }
        };

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

        // Load disease clusters for context-aware clustering.
        let _diseases = super::phase::load_diseases(&ctx.sipag_dir, &repo.full_name);

        // TODO Phase 5: Implement issue clustering and PR creation
        // 1. Read all ready issue bodies
        // 2. Invoke Claude to cluster by disease
        // 3. For each cluster: create branch, create PR, dispatch worker
        // 4. Label transition: ready → in-progress
    }

    eprintln!("sipag: poll cycle complete");
    Ok(())
}
