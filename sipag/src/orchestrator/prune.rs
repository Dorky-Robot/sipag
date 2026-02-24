use anyhow::Result;

use super::phase::SessionState;
use super::OrchestratorContext;

/// Prune stale issues for a single repo.
///
/// Cross-references every open issue against the actual codebase to find issues
/// that reference files, functions, or patterns that no longer exist. Closes
/// stale issues with a brief explanation.
///
/// Mechanical checks (file existence) are pure code. Ambiguous cases are
/// delegated to Claude via `invoke_claude()`.
pub fn run_prune(
    repo_index: usize,
    _session: &mut SessionState,
    ctx: &OrchestratorContext,
) -> Result<()> {
    let repo = &ctx.repos[repo_index];
    eprintln!("sipag: pruning stale issues for {}", repo.full_name);

    // TODO Phase 5: Implement stale issue detection
    // 1. Fetch all open issues: gh issue list --repo <repo> --state open --json number,title,body
    // 2. For each issue, check if referenced files/modules still exist (Glob/stat)
    // 3. For ambiguous cases, invoke Claude to determine staleness
    // 4. Close stale issues: github::close_issue(repo, num, comment)

    Ok(())
}
