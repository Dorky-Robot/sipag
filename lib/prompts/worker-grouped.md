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

2. **Map the concern topology before coding.** Issues don't always cluster neatly — some sit at the intersection of multiple concern circles (like a Venn diagram).
   - Identify the dominant concern cluster among these issues. What is the shared theme?
   - Which issues fit squarely in that cluster? Which sit at intersections with other concerns?
   - An issue at the intersection of "error handling" and "config validation" belongs to both clusters. When batched with an error-handling group, address the error-handling facet. Leave the config facet for a config-focused batch.

3. **Address intersection issues through the lens of the dominant cluster.** If this batch is primarily about error handling, address intersection issues from that angle — don't try to resolve all their facets at once. The other facets will be addressed when those issues appear in the appropriate cluster.

4. **Design cohesive changes.** Prefer one well-designed PR over isolated patches. A single PR that addresses 3 issues with a unified approach is better than 3 mechanical fixes.

5. **You don't have to address every issue.** If an issue doesn't fit naturally with the others, skip it — it will be picked up in a later cycle. Only claim issues you actually resolve.

6. **Partial addressing is fine — but be honest about it.** Use `Closes #N` only for issues that are **fully resolved** by this PR. If you addressed one facet of an issue but not others:
   - Do NOT use `Closes #N` for that issue
   - Leave a comment on the issue explaining what was addressed and what remains
   - Example: "Addressed the error-display aspect in PR #X. The config-validation aspect remains open for a config-focused batch."

7. **Name the clusters in your PR description.** State explicitly:
   - The dominant concern cluster this PR addresses (e.g., "error-handling cluster")
   - Which issues are fully addressed (`Closes #N`)
   - Which issues are partially addressed and what remains

8. **Branch and PR are already set up.** You are on branch `{{BRANCH}}` — do NOT create a new branch. A draft PR is already open for this branch — do not open another one.

9. **Validate your changes:**
   - Run `make dev` (fmt + clippy + test) before committing
   - Run any existing tests and make sure they pass
   - Commit with a clear message explaining the unified approach
   - Push to origin

The PR will be marked ready for review automatically when you finish.
