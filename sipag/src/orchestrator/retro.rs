use anyhow::Result;

use super::OrchestratorContext;

/// Run a self-improvement retrospective using 3 parallel Claude agents.
///
/// Agents: operator retro, design retro, correctness retro.
/// Results are synthesized, deduplicated, and recorded as lessons.
pub fn run_retro(ctx: &OrchestratorContext) -> Result<()> {
    eprintln!("sipag: running retrospective");

    // TODO Phase 5: Implement retro
    // 1. Gather cycle data from sipag ps, events/, lessons/
    // 2. Build 3 parallel ClaudeInvocations (operator, design, correctness)
    // 3. invoke_claude_parallel()
    // 4. Synthesize findings
    // 5. For sipag infrastructure fixes: apply directly
    // 6. Record retro summary to lessons/sipag.md

    let _ = ctx;
    eprintln!("sipag: retrospective complete (stub)");
    Ok(())
}
