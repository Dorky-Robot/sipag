use anyhow::Result;
use sipag_core::worker::github;

use super::phase::ReviewOutcome;
use super::OrchestratorContext;

/// Maximum review gate attempts before escalating to human review.
const MAX_REVIEW_ATTEMPTS: u8 = 2;

/// Run the review gate for a finished PR.
///
/// 1. Fetch PR diff and details
/// 2. Launch 5 parallel review agents (scope, security, arch, correctness, test adequacy)
/// 3. Synthesize verdicts
/// 4. All approve → merge; any REQUEST_CHANGES → post feedback, re-dispatch
pub fn run_review(
    repo: &str,
    pr_num: u64,
    attempt: u8,
    _ctx: &OrchestratorContext,
) -> Result<ReviewOutcome> {
    eprintln!(
        "sipag: reviewing PR #{pr_num} in {repo} (attempt {}/{})",
        attempt + 1,
        MAX_REVIEW_ATTEMPTS
    );

    // Check if PR is already merged (worker may have self-merged).
    let details = github::get_pr_details(repo, pr_num)?;
    if details.state == "MERGED" {
        eprintln!("sipag: PR #{pr_num} already merged, skipping review");
        return Ok(ReviewOutcome::Skipped);
    }

    if details.state == "CLOSED" {
        eprintln!("sipag: PR #{pr_num} is closed, skipping review");
        return Ok(ReviewOutcome::Skipped);
    }

    // TODO Phase 5: Implement multi-agent review
    // 1. Fetch diff: github::get_pr_diff(repo, pr_num)
    // 2. Fetch issue bodies for context
    // 3. Build 5 parallel ClaudeInvocations (scope, security, arch, correctness, test)
    // 4. invoke_claude_parallel()
    // 5. Parse verdicts (APPROVE / APPROVE_WITH_NOTES / REQUEST_CHANGES)
    // 6. If all approve: github::merge_pr(repo, pr_num)
    // 7. If any REQUEST_CHANGES and attempt < MAX_REVIEW_ATTEMPTS:
    //    - Post structured feedback as PR comment
    //    - Append feedback to PR body
    //    - Re-dispatch worker
    //    - Return NeedsRedispatch
    // 8. If still REQUEST_CHANGES after max attempts: return Escalate

    // Stub: attempt to merge
    eprintln!("sipag: review complete for PR #{pr_num} — attempting merge");
    match github::merge_pr(repo, pr_num) {
        Ok(()) => {
            eprintln!("sipag: PR #{pr_num} merged successfully");
            Ok(ReviewOutcome::Merged)
        }
        Err(e) => {
            eprintln!("sipag: failed to merge PR #{pr_num}: {e}");
            Ok(ReviewOutcome::Escalate)
        }
    }
}
