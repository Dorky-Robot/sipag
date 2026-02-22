You are working on the repository at /work.

You have been given multiple related issues to address. Read them all before starting — they may share root causes, overlap, or benefit from a unified approach.

## Issues

{{ISSUES}}

## Instructions

1. **Think holistically.** These issues were grouped because they're all ready at the same time. Look for:
   - Shared root causes — one fix may resolve several issues
   - Overlapping changes — touching the same files or modules
   - Contradictions — where issues conflict, choose the more coherent direction
   - The deeper "why" — what are maintainers really trying to achieve?

2. **Map the concern topology before writing any code.**

   Think of each issue as a point on a Venn diagram. Before touching a file, ask: what are the dominant *concern clusters* in this batch?

   - **Identify clusters.** Group issues by their primary concern (e.g., "error handling", "config", "logging"). Most batches have one clear dominant cluster — implement it fully.
   - **Recognize intersections.** Some issues sit at the boundary of two concerns (e.g., "config errors should show friendly messages" belongs to both "config" and "error handling"). When an issue touches multiple concerns:
     - Address only the facet that aligns with *this batch's dominant theme*.
     - Leave the other facet for a separate cycle where it becomes the primary concern.
   - **State the cluster explicitly.** In your PR description, name the dominant concern cluster (e.g., "This PR addresses the error-handling cluster: #326, #327, #328").

3. **Design cohesive changes.** Prefer one well-designed PR over isolated patches. A single PR that addresses 3 issues with a unified approach is better than 3 mechanical fixes.

4. **You don't have to address every issue.** If an issue doesn't fit naturally with the dominant theme, skip it — it will be picked up in a later cycle. Only claim issues you actually resolve.

5. **Partial addressing is fine — but be honest about it.**
   - Use `Closes #N` **only** when the issue is *fully* resolved by this PR.
   - If you addressed one facet of an issue but not others, **do not** use `Closes #N`. Instead, post a comment on the issue explaining: what was addressed, through which lens (e.g., "addressed the error-handling UX; config semantics remain"), and what is left for a future cycle.
   - This prevents the PR body from overpromising and avoids issues being incorrectly marked closed.

6. **Branch and PR are already set up.** You are on branch `{{BRANCH}}` — do NOT create a new branch. A draft PR is already open for this branch — do not open another one.

7. **Mark what you addressed.** In your commits and the PR description:
   - Use `Closes #N` only for fully resolved issues.
   - List partially addressed issues separately, with a note on what remains.
   - State the dominant cluster name so reviewers understand the scope.

8. **Validate your changes:**
   - Run `make dev` (fmt + clippy + test) before committing
   - Run any existing tests and make sure they pass
   - Commit with a clear message explaining the unified approach
   - Push to origin

9. **Update the PR description.** After pushing, update the PR body with a structured summary using `gh pr edit <branch> --repo <repo> --body <body>`. The description must include:
   - **Cluster name**: The dominant concern addressed (e.g., "This PR addresses the worker lifecycle cluster")
   - **Per-issue summary**: For each issue addressed, a 1-2 sentence explanation of what was done
   - **Issues NOT addressed**: List any issues from the batch that were skipped and why
   - **Test plan**: How the changes were validated
   - Keep `Closes #N` lines at the top for issues fully resolved

   Example format:
   ```
   Closes #101
   Closes #103

   ## Cluster: error handling

   - **#101** — Added retry logic to API calls with exponential backoff
   - **#103** — Unified error messages to use structured format with error codes
   - **#102** — Not addressed (config concern, better suited for a dedicated config PR)

   ## Test plan
   - `make dev` passes (fmt + clippy + all tests)
   - Manual test: API timeout triggers retry and succeeds on second attempt
   ```

The PR will be marked ready for review automatically when you finish.
