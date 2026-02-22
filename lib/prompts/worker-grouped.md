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

2. **Design cohesive changes.** Prefer one well-designed PR over isolated patches. A single PR that addresses 3 issues with a unified approach is better than 3 mechanical fixes.

3. **You don't have to address every issue.** If an issue doesn't fit naturally with the others, skip it — it will be picked up in a later cycle. Only claim issues you actually resolve.

4. **Branch and PR are already set up.** You are on branch `{{BRANCH}}` — do NOT create a new branch. A draft PR is already open for this branch — do not open another one.

5. **Mark what you addressed.** In your commits and the PR description, use `Closes #N` only for issues you actually resolved. Issues without `Closes #N` will remain open for future workers.

6. **Validate your changes:**
   - Run `make dev` (fmt + clippy + test) before committing
   - Run any existing tests and make sure they pass
   - Commit with a clear message explaining the unified approach
   - Push to origin

The PR will be marked ready for review automatically when you finish.
