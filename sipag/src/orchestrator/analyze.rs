use anyhow::Result;

use super::phase::SessionState;
use super::OrchestratorContext;

/// Run disease analysis for a single repo using 4 parallel Claude agents.
///
/// Agents: security, architecture, code quality, testing.
/// Each receives the pruned issue list plus codebase access and returns
/// structured disease findings. Results are deduplicated, ranked, and stored
/// in `session.diseases`.
pub fn run_analyze(
    repo_index: usize,
    _session: &mut SessionState,
    ctx: &OrchestratorContext,
) -> Result<()> {
    let repo = &ctx.repos[repo_index];
    eprintln!("sipag: analyzing diseases for {}", repo.full_name);

    // TODO Phase 5: Implement disease analysis
    // 1. Fetch pruned issue list
    // 2. Build 4 parallel ClaudeInvocations (security, arch, quality, testing)
    //    with --allowedTools Read,Glob,Grep and working_dir set to repo.local_path
    // 3. invoke_claude_parallel()
    // 4. extract_json() for each result into DiseaseCluster format
    // 5. Deduplicate and rank by impact
    // 6. Store in session.diseases

    Ok(())
}
