use anyhow::Result;
use sipag_core::worker::{dispatch, github, lifecycle};

use super::OrchestratorContext;

/// Handle a failed worker: check logs, triage, possibly re-dispatch.
pub fn handle_failed(repo: &str, pr_num: u64, ctx: &OrchestratorContext) -> Result<()> {
    eprintln!("sipag: handling failed worker for PR #{pr_num} in {repo}");

    // Check logs for failure reason.
    let repo_slug = repo.replace('/', "--");
    let log_path = ctx
        .sipag_dir
        .join("logs")
        .join(format!("{repo_slug}--pr-{pr_num}.log"));

    let reason = dispatch::extract_failure_reason(&log_path)
        .unwrap_or_else(|| "unknown failure".to_string());
    eprintln!("sipag: failure reason: {reason}");

    // Write escalation event.
    let _ = sipag_core::events::write_event(
        &ctx.sipag_dir,
        "worker-failed",
        repo,
        &format!("Worker failed for PR #{pr_num} in {repo}"),
        &reason,
    );

    // TODO Phase 5: Invoke Claude to triage failure
    // - Transient (network, timeout) → re-dispatch
    // - Permanent (auth, repo not found) → escalate
    // - Code issue → post feedback on PR, re-dispatch

    Ok(())
}

/// Handle a stale worker: kill container, check PR state, possibly re-dispatch.
pub fn handle_stale(repo: &str, pr_num: u64, ctx: &OrchestratorContext) -> Result<()> {
    eprintln!("sipag: handling stale worker for PR #{pr_num} in {repo}");

    // Kill the stale container.
    let workers = lifecycle::scan_workers(&ctx.sipag_dir);
    if let Some(w) = workers
        .iter()
        .find(|w| w.repo == repo && w.pr_num == pr_num)
    {
        if !w.container_id.is_empty() {
            let _ = std::process::Command::new("docker")
                .args(["kill", &w.container_id])
                .output();
            eprintln!("sipag: killed stale container {}", w.container_id);
        }
    }

    // Check PR state — if it has real commits, consider re-dispatch.
    match github::get_pr_details(repo, pr_num) {
        Ok(details) => {
            if details.state == "MERGED" || details.state == "CLOSED" {
                eprintln!("sipag: PR #{pr_num} is {}, no action needed", details.state);
                return Ok(());
            }

            eprintln!("sipag: PR #{pr_num} is still open — will be re-dispatched on next poll");

            // TODO Phase 4: Re-dispatch the worker
            // dispatch::dispatch_worker(repo, pr_num, &details.head_ref, &[], &ctx.cfg, &ctx.creds)?;
        }
        Err(e) => {
            eprintln!("sipag: failed to check PR #{pr_num} state: {e}");
        }
    }

    Ok(())
}
