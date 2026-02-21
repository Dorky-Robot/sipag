//! Pure prompt-building functions for worker containers.
//!
//! All functions are pure (no I/O, no side effects) and operate only on
//! their inputs. Template placeholders use `{{KEY}}` syntax matching the
//! files in `lib/prompts/`.

/// The result of building an issue worker prompt.
///
/// Pure value — no I/O performed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuePrompt {
    /// The prompt to pass to Claude inside the container.
    pub prompt: String,
    /// The branch the container should check out / push to.
    pub branch: String,
    /// Draft PR body (used when creating the PR before work starts).
    pub pr_body: String,
}

/// Build the prompt for a new issue worker.
///
/// Substitutes `{{TITLE}}`, `{{BODY}}`, `{{BRANCH}}`, `{{ISSUE_NUM}}` in the
/// template and constructs a standard PR body. Returns a fully resolved
/// [`IssuePrompt`].
///
/// Pure — no side effects.
pub fn build_issue_prompt(
    template: &str,
    title: &str,
    body: &str,
    branch: &str,
    issue_num: u64,
) -> IssuePrompt {
    let prompt = template
        .replace("{{TITLE}}", title)
        .replace("{{BODY}}", body)
        .replace("{{BRANCH}}", branch)
        .replace("{{ISSUE_NUM}}", &issue_num.to_string());

    let pr_body = format!(
        "Closes #{issue_num}\n\n{body}\n\n---\n\
         *This PR was opened by a sipag worker. \
         Commits will appear as work progresses.*"
    );

    IssuePrompt {
        prompt,
        branch: branch.to_string(),
        pr_body,
    }
}

/// Build the prompt for a PR iteration worker.
///
/// Substitutes `{{PR_NUM}}`, `{{REPO}}`, `{{ISSUE_BODY}}`, `{{PR_DIFF}}`,
/// `{{REVIEW_FEEDBACK}}`, `{{BRANCH}}` in the template.
///
/// Pure — no side effects.
pub fn build_iteration_prompt(
    template: &str,
    pr_num: u64,
    repo: &str,
    issue_body: &str,
    pr_diff: &str,
    review_feedback: &str,
    branch: &str,
) -> String {
    template
        .replace("{{PR_NUM}}", &pr_num.to_string())
        .replace("{{REPO}}", repo)
        .replace("{{ISSUE_BODY}}", issue_body)
        .replace("{{PR_DIFF}}", pr_diff)
        .replace("{{REVIEW_FEEDBACK}}", review_feedback)
        .replace("{{BRANCH}}", branch)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ISSUE_TEMPLATE: &str =
        "Task: {{TITLE}}\n\n{{BODY}}\n\nBranch: {{BRANCH}}\nIssue: #{{ISSUE_NUM}}";

    const ITERATION_TEMPLATE: &str =
        "PR #{{PR_NUM}} in {{REPO}}\nIssue: {{ISSUE_BODY}}\nDiff: {{PR_DIFF}}\n\
         Feedback: {{REVIEW_FEEDBACK}}\nBranch: {{BRANCH}}";

    // ── build_issue_prompt ────────────────────────────────────────────────────

    #[test]
    fn build_issue_prompt_substitutes_all_placeholders() {
        let result = build_issue_prompt(
            ISSUE_TEMPLATE,
            "Fix the bug",
            "Issue body here",
            "sipag/issue-42-fix-the-bug",
            42,
        );
        assert_eq!(
            result.prompt,
            "Task: Fix the bug\n\nIssue body here\n\n\
             Branch: sipag/issue-42-fix-the-bug\nIssue: #42"
        );
    }

    #[test]
    fn build_issue_prompt_returns_branch() {
        let result = build_issue_prompt(
            ISSUE_TEMPLATE,
            "Fix the bug",
            "body",
            "sipag/issue-42-fix-the-bug",
            42,
        );
        assert_eq!(result.branch, "sipag/issue-42-fix-the-bug");
    }

    #[test]
    fn build_issue_prompt_generates_pr_body_with_closes() {
        let result = build_issue_prompt(
            ISSUE_TEMPLATE,
            "Fix the bug",
            "Issue body here",
            "sipag/issue-42-fix-the-bug",
            42,
        );
        assert!(result.pr_body.contains("Closes #42"));
        assert!(result.pr_body.contains("Issue body here"));
        assert!(result.pr_body.contains("sipag worker"));
    }

    #[test]
    fn build_issue_prompt_handles_empty_body() {
        let result = build_issue_prompt(
            ISSUE_TEMPLATE,
            "Fix the bug",
            "",
            "sipag/issue-42-fix-the-bug",
            42,
        );
        assert!(result.prompt.contains("Fix the bug"));
        assert!(result.pr_body.contains("Closes #42"));
    }

    #[test]
    fn build_issue_prompt_no_leftover_placeholders() {
        let result = build_issue_prompt(
            ISSUE_TEMPLATE,
            "My Title",
            "My body",
            "sipag/issue-1-my-title",
            1,
        );
        assert!(!result.prompt.contains("{{"));
        assert!(!result.pr_body.contains("{{"));
    }

    #[test]
    fn build_issue_prompt_different_issue_numbers() {
        let r1 = build_issue_prompt(ISSUE_TEMPLATE, "A", "B", "branch", 1);
        let r999 = build_issue_prompt(ISSUE_TEMPLATE, "A", "B", "branch", 999);
        assert!(r1.prompt.contains("#1"));
        assert!(r999.prompt.contains("#999"));
        assert!(r1.pr_body.contains("Closes #1"));
        assert!(r999.pr_body.contains("Closes #999"));
    }

    // ── build_iteration_prompt ────────────────────────────────────────────────

    #[test]
    fn build_iteration_prompt_substitutes_all_placeholders() {
        let result = build_iteration_prompt(
            ITERATION_TEMPLATE,
            123,
            "owner/repo",
            "Original issue body",
            "diff content here",
            "Please fix X and Y",
            "sipag/issue-42-feature",
        );
        assert!(result.contains("PR #123"));
        assert!(result.contains("owner/repo"));
        assert!(result.contains("Original issue body"));
        assert!(result.contains("diff content here"));
        assert!(result.contains("Please fix X and Y"));
        assert!(result.contains("sipag/issue-42-feature"));
    }

    #[test]
    fn build_iteration_prompt_no_leftover_placeholders() {
        let result = build_iteration_prompt(
            ITERATION_TEMPLATE,
            1,
            "a/b",
            "body",
            "diff",
            "feedback",
            "branch",
        );
        assert!(!result.contains("{{"));
        assert!(!result.contains("}}"));
    }

    #[test]
    fn build_iteration_prompt_empty_fields_allowed() {
        // Empty pr_diff and review_feedback are valid (edge case for new PRs)
        let result = build_iteration_prompt(
            ITERATION_TEMPLATE,
            5,
            "owner/repo",
            "body",
            "",
            "",
            "my-branch",
        );
        assert!(result.contains("PR #5"));
        assert!(!result.contains("{{"));
    }
}
