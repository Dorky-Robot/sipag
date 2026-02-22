//! Pre-dispatch brainstorming: 3 parallel perspectives synthesized into one plan.
//!
//! Runs host-side `claude --print` calls (like `triage.rs`) to expand
//! possibilities before narrowing. Three agents propose different PR
//! strategies, then a synthesis call produces one focused implementation plan
//! that gets injected into the worker prompt.
//!
//! This is best-effort — if all agents fail or `claude` is unavailable,
//! dispatch proceeds without a plan (current behavior).

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Instant;

/// Result of the issue selection funnel (Stage 0-1).
pub(crate) struct FunnelResult {
    /// Issue numbers in the selected cluster (always includes ready issues).
    pub selected_issues: Vec<u64>,
    /// Brief rationale for why this cluster was chosen.
    pub cluster_rationale: String,
    /// Wall-clock duration of the funnel phase.
    pub duration_secs: u64,
}

/// Result of the brainstorm phase.
pub(crate) struct BrainstormResult {
    /// The synthesized plan text, ready for injection into the worker prompt.
    pub plan: String,
    /// Wall-clock duration of the entire brainstorm phase.
    pub duration_secs: u64,
}

/// Run the brainstorm phase: 3 parallel perspective agents + 1 synthesis call.
///
/// Returns `None` if brainstorming is skipped (all agents fail, `claude` not
/// available, etc.). Never returns an error — failures are logged and swallowed.
pub(crate) fn run_brainstorm(
    all_issues_section: &str,
    ready_issues_section: &str,
) -> Option<BrainstormResult> {
    let start = Instant::now();

    // Build prompts for each perspective.
    let arch_prompt = build_architectural_prompt(all_issues_section, ready_issues_section);
    let delivery_prompt = build_delivery_prompt(all_issues_section, ready_issues_section);
    let risk_prompt = build_risk_prompt(all_issues_section, ready_issues_section);

    // Run 3 perspectives in parallel using std::thread::scope.
    let (arch_result, delivery_result, risk_result) = std::thread::scope(|s| {
        let h_arch = s.spawn(|| run_claude_print(&arch_prompt));
        let h_delivery = s.spawn(|| run_claude_print(&delivery_prompt));
        let h_risk = s.spawn(|| run_claude_print(&risk_prompt));

        (
            h_arch.join().unwrap_or(None),
            h_delivery.join().unwrap_or(None),
            h_risk.join().unwrap_or(None),
        )
    });

    // Collect successful proposals.
    let mut proposals: Vec<(&str, String)> = Vec::new();
    if let Some(text) = arch_result {
        proposals.push(("Architectural Coherence", text));
    }
    if let Some(text) = delivery_result {
        proposals.push(("Practical Delivery", text));
    }
    if let Some(text) = risk_result {
        proposals.push(("Risk & Dependency", text));
    }

    if proposals.is_empty() {
        eprintln!("[brainstorm] All 3 perspectives failed — proceeding without plan");
        return None;
    }

    println!(
        "[brainstorm] {}/3 perspectives completed in {}s",
        proposals.len(),
        start.elapsed().as_secs()
    );

    // Synthesize into one plan.
    let synthesis_prompt = build_synthesis_prompt(&proposals);
    let synthesis = run_claude_print(&synthesis_prompt);

    let plan = match synthesis {
        Some(text) => text,
        None => {
            eprintln!("[brainstorm] Synthesis failed — proceeding without plan");
            return None;
        }
    };

    let duration_secs = start.elapsed().as_secs();
    println!("[brainstorm] Synthesis complete — total {}s", duration_secs);

    Some(BrainstormResult {
        plan,
        duration_secs,
    })
}

/// Format a successful brainstorm plan for injection into the worker prompt.
pub(crate) fn format_brainstorm_section(plan: &str) -> String {
    format!(
        "## Pre-analysis (brainstorm synthesis)\n\n\
         A team of analysts reviewed these issues from three perspectives\n\
         (architectural coherence, practical delivery, risk/dependency).\n\
         Here is their synthesized recommendation:\n\n\
         {plan}\n\n\
         **Note:** This analysis is advisory. Use your own judgment — you have\n\
         access to the actual codebase and may discover better approaches.\n"
    )
}

// ── Perspective prompts ──────────────────────────────────────────────────────

fn build_architectural_prompt(all_issues: &str, ready_issues: &str) -> String {
    format!(
        "You are an architectural coherence analyst reviewing a project's issue backlog.\n\n\
         ## All open issues\n\n{all_issues}\n\n\
         ## Issues ready for work\n\n{ready_issues}\n\n\
         ## Your task\n\n\
         Analyze these issues through an **architectural coherence** lens:\n\
         - Which issues share a root cause or missing abstraction?\n\
         - What single architectural change addresses the most issues?\n\
         - Are there implicit patterns that suggest a structural gap?\n\n\
         Produce a structured proposal (~500 words max):\n\
         1. **Recommended approach**: The unifying architectural insight\n\
         2. **Issues to address**: Which ready issues this approach covers (by number)\n\
         3. **Key changes**: What files/modules would change and how\n\
         4. **Risks**: What could go wrong with this approach\n"
    )
}

fn build_delivery_prompt(all_issues: &str, ready_issues: &str) -> String {
    format!(
        "You are a practical delivery analyst reviewing a project's issue backlog.\n\n\
         ## All open issues\n\n{all_issues}\n\n\
         ## Issues ready for work\n\n{ready_issues}\n\n\
         ## Your task\n\n\
         Analyze these issues through a **practical delivery** lens:\n\
         - What's the highest-value PR that can ship in one session?\n\
         - What's the right scope — not too big (won't finish), not too small (low impact)?\n\
         - Which combination of issues produces the most cohesive change?\n\n\
         Produce a structured proposal (~500 words max):\n\
         1. **Recommended approach**: The most impactful deliverable\n\
         2. **Issues to address**: Which ready issues to tackle (by number)\n\
         3. **Key changes**: Concrete implementation steps\n\
         4. **Risks**: Scope creep dangers, testing concerns\n"
    )
}

fn build_risk_prompt(all_issues: &str, ready_issues: &str) -> String {
    format!(
        "You are a risk and dependency analyst reviewing a project's issue backlog.\n\n\
         ## All open issues\n\n{all_issues}\n\n\
         ## Issues ready for work\n\n{ready_issues}\n\n\
         ## Your task\n\n\
         Analyze these issues through a **risk and dependency** lens:\n\
         - Which issues have implicit dependencies on each other?\n\
         - What ordering minimizes merge conflicts and rework?\n\
         - Which issues are \"load-bearing\" — blocking future progress?\n\
         - Are there hidden prerequisites that aren't captured as issues?\n\n\
         Produce a structured proposal (~500 words max):\n\
         1. **Recommended approach**: The safest, most efficient ordering\n\
         2. **Issues to address**: Which ready issues to start with (by number)\n\
         3. **Key changes**: What to do and in what order\n\
         4. **Risks**: Dependencies, conflicts, and things to watch out for\n"
    )
}

// ── Synthesis prompt ─────────────────────────────────────────────────────────

fn build_synthesis_prompt(proposals: &[(&str, String)]) -> String {
    let mut prompt = String::from(
        "You are a technical lead synthesizing three analyst proposals into one implementation plan.\n\n\
         Three analysts reviewed the same issue backlog from different perspectives. \
         Here are their proposals:\n\n",
    );

    for (name, text) in proposals {
        prompt.push_str(&format!("### {name} perspective\n\n{text}\n\n"));
    }

    prompt.push_str(
        "## Your task\n\n\
         Synthesize these proposals into ONE implementation plan (~600 words max).\n\
         Find the common ground and resolve conflicts. Produce:\n\n\
         1. **Goal**: One sentence describing what this PR achieves\n\
         2. **Issues addressed**: List with `Closes #N` or `Partially addresses #N` \
            for each issue touched\n\
         3. **Approach**: Numbered implementation steps\n\
         4. **Key files**: Which files will be modified and why\n\
         5. **Testing strategy**: How to validate the changes\n\
         6. **Out of scope**: What to explicitly NOT do in this PR\n",
    );

    prompt
}

// ── Claude CLI wrapper ───────────────────────────────────────────────────────

/// Run `claude --print` with the given prompt, returning stdout on success.
///
/// Pipes the prompt via stdin to avoid OS argument size limits (E2BIG).
/// Returns `None` on any failure (missing binary, non-zero exit, timeout, etc.).
fn run_claude_print(prompt: &str) -> Option<String> {
    let mut child = match Command::new("claude")
        .args(["--print"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[brainstorm] Failed to spawn claude: {e}");
            return None;
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(prompt.as_bytes()) {
            eprintln!("[brainstorm] Failed to write prompt to stdin: {e}");
            return None;
        }
    }

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[brainstorm] Failed to wait for claude: {e}");
            return None;
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!(
            "[brainstorm] claude exited with status {}: {}",
            output.status.code().unwrap_or(-1),
            stderr.trim()
        );
        return None;
    }

    let text = String::from_utf8(output.stdout).ok()?;
    if text.trim().is_empty() {
        eprintln!("[brainstorm] claude returned empty output");
        return None;
    }

    Some(text)
}

// ── Issue selection funnel ──────────────────────────────────────────────────

/// Run the issue selection funnel: cluster titles, pick the best cluster.
///
/// Takes all issue titles (cheap — no bodies) and the ready issue numbers.
/// Returns the selected issue numbers for detailed fetching.
///
/// Returns `None` if the funnel fails — caller falls back to fetching all issues.
pub(crate) fn run_issue_funnel(
    all_titles: &[(u64, String)],
    ready_issue_nums: &[u64],
) -> Option<FunnelResult> {
    let start = Instant::now();

    let prompt = build_funnel_prompt(all_titles, ready_issue_nums);
    let response = run_claude_print(&prompt)?;

    let mut result = parse_funnel_response(&response, ready_issue_nums)?;
    result.duration_secs = start.elapsed().as_secs();

    println!(
        "[funnel] Selected {} issues from {} total in {}s",
        result.selected_issues.len(),
        all_titles.len(),
        result.duration_secs
    );

    Some(result)
}

fn build_funnel_prompt(all_titles: &[(u64, String)], ready_issue_nums: &[u64]) -> String {
    let mut prompt = String::from(
        "You are a technical project manager reviewing an issue backlog.\n\n\
         ## All open issues (titles only)\n\n",
    );

    for (num, title) in all_titles {
        let ready_marker = if ready_issue_nums.contains(num) {
            " [READY]"
        } else {
            ""
        };
        prompt.push_str(&format!("- #{num}: {title}{ready_marker}\n"));
    }

    let ready_refs: Vec<String> = ready_issue_nums.iter().map(|n| format!("#{n}")).collect();
    prompt.push_str(&format!(
        "\n## Ready issues (must be included)\n\n{}\n\n",
        ready_refs.join(", ")
    ));

    prompt.push_str(
        "## Your task\n\n\
         1. **Cluster** these issues by theme (shared root cause, related subsystem, \
            common abstraction, or dependency chain). Name each cluster.\n\
         2. **Select** the single highest bang-for-buck cluster — the one where addressing \
            a few issues together produces the most cohesive, impactful change. \
            Issues marked [READY] MUST be included in the selected cluster (merge them \
            into whichever cluster they best fit).\n\
         3. **Output** ONLY a JSON object. No explanation before or after it.\n\n\
         Required format:\n\
         ```json\n\
         {\n\
           \"clusters\": [\n\
             {\"name\": \"Error handling\", \"issues\": [101, 103, 107]},\n\
             {\"name\": \"Worker lifecycle\", \"issues\": [110, 115, 120]}\n\
           ],\n\
           \"selected_cluster\": \"Error handling\",\n\
           \"selected_issues\": [101, 103, 107],\n\
           \"rationale\": \"These share a missing unified error type — one change addresses all three.\"\n\
         }\n\
         ```\n\n\
         Keep the selected cluster to 10-20 issues max. Include the ready issues plus \
         the most related issues from the backlog.\n",
    );

    prompt
}

/// Parse the funnel response, ensuring ready issues are always included.
fn parse_funnel_response(response: &str, ready_issue_nums: &[u64]) -> Option<FunnelResult> {
    let start = response.find('{')?;
    let end = response.rfind('}')?;
    if end < start {
        return None;
    }
    let json_str = &response[start..=end];
    let parsed: serde_json::Value = serde_json::from_str(json_str).ok()?;

    let mut selected: Vec<u64> = parsed["selected_issues"]
        .as_array()?
        .iter()
        .filter_map(|v| v.as_u64())
        .collect();

    // Ensure all ready issues are included (safety net).
    for &num in ready_issue_nums {
        if !selected.contains(&num) {
            selected.push(num);
        }
    }

    let rationale = parsed["rationale"].as_str().unwrap_or("").to_string();

    Some(FunnelResult {
        selected_issues: selected,
        cluster_rationale: rationale,
        duration_secs: 0,
    })
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_brainstorm_section_wraps_plan() {
        let section = format_brainstorm_section("Here is the plan.");
        assert!(section.contains("Pre-analysis"));
        assert!(section.contains("Here is the plan."));
        assert!(section.contains("advisory"));
    }

    #[test]
    fn format_brainstorm_section_mentions_three_perspectives() {
        let section = format_brainstorm_section("test");
        assert!(section.contains("architectural coherence"));
        assert!(section.contains("practical delivery"));
        assert!(section.contains("risk/dependency"));
    }

    #[test]
    fn architectural_prompt_contains_issues() {
        let prompt = build_architectural_prompt("issue #1 text", "ready #2 text");
        assert!(prompt.contains("issue #1 text"));
        assert!(prompt.contains("ready #2 text"));
        assert!(prompt.contains("architectural coherence"));
    }

    #[test]
    fn delivery_prompt_contains_issues() {
        let prompt = build_delivery_prompt("issue #1 text", "ready #2 text");
        assert!(prompt.contains("issue #1 text"));
        assert!(prompt.contains("ready #2 text"));
        assert!(prompt.contains("practical delivery"));
    }

    #[test]
    fn risk_prompt_contains_issues() {
        let prompt = build_risk_prompt("issue #1 text", "ready #2 text");
        assert!(prompt.contains("issue #1 text"));
        assert!(prompt.contains("ready #2 text"));
        assert!(prompt.contains("risk and dependency"));
    }

    #[test]
    fn synthesis_prompt_includes_all_proposals() {
        let proposals = vec![
            ("Agent A", "Proposal A content".to_string()),
            ("Agent B", "Proposal B content".to_string()),
        ];
        let prompt = build_synthesis_prompt(&proposals);
        assert!(prompt.contains("Agent A"));
        assert!(prompt.contains("Proposal A content"));
        assert!(prompt.contains("Agent B"));
        assert!(prompt.contains("Proposal B content"));
        assert!(prompt.contains("Goal"));
        assert!(prompt.contains("Out of scope"));
    }

    #[test]
    fn synthesis_prompt_with_single_proposal() {
        let proposals = vec![("Only One", "Solo proposal".to_string())];
        let prompt = build_synthesis_prompt(&proposals);
        assert!(prompt.contains("Only One"));
        assert!(prompt.contains("Solo proposal"));
    }

    // ── Funnel tests ────────────────────────────────────────────────────────

    #[test]
    fn funnel_prompt_lists_titles_with_ready_markers() {
        let titles = vec![
            (1, "Fix auth bug".to_string()),
            (2, "Add dark mode".to_string()),
            (3, "Refactor config".to_string()),
        ];
        let ready = vec![1, 3];
        let prompt = build_funnel_prompt(&titles, &ready);
        assert!(prompt.contains("- #1: Fix auth bug [READY]"));
        assert!(prompt.contains("- #2: Add dark mode\n"));
        assert!(prompt.contains("- #3: Refactor config [READY]"));
        assert!(prompt.contains("#1, #3"));
    }

    #[test]
    fn funnel_prompt_with_no_ready_issues() {
        let titles = vec![(10, "Some issue".to_string())];
        let ready: Vec<u64> = vec![];
        let prompt = build_funnel_prompt(&titles, &ready);
        assert!(prompt.contains("- #10: Some issue\n"));
        // No title line should have the [READY] marker.
        assert!(!prompt.contains("Some issue [READY]"));
    }

    #[test]
    fn parse_funnel_valid_json() {
        let response = r#"{"clusters":[{"name":"A","issues":[1,2]},{"name":"B","issues":[3]}],"selected_cluster":"A","selected_issues":[1,2],"rationale":"Best cluster"}"#;
        let result = parse_funnel_response(response, &[1]).unwrap();
        assert_eq!(result.selected_issues, vec![1, 2]);
        assert_eq!(result.cluster_rationale, "Best cluster");
    }

    #[test]
    fn parse_funnel_ensures_ready_issues_included() {
        let response = r#"{"selected_issues":[5,6],"rationale":"picked"}"#;
        let result = parse_funnel_response(response, &[5, 99]).unwrap();
        assert!(result.selected_issues.contains(&5));
        assert!(result.selected_issues.contains(&99));
        assert!(result.selected_issues.contains(&6));
    }

    #[test]
    fn parse_funnel_malformed_json_returns_none() {
        assert!(parse_funnel_response("not json at all", &[1]).is_none());
    }

    #[test]
    fn parse_funnel_missing_selected_issues_returns_none() {
        let response = r#"{"rationale":"no issues field"}"#;
        assert!(parse_funnel_response(response, &[1]).is_none());
    }

    #[test]
    fn parse_funnel_json_embedded_in_prose() {
        let response = "Here is the result:\n```json\n{\"selected_issues\":[10,20],\"rationale\":\"good\"}\n```\nDone.";
        let result = parse_funnel_response(response, &[10]).unwrap();
        assert_eq!(result.selected_issues, vec![10, 20]);
    }
}
