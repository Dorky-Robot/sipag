## Hard constraints

- You are working on branch `{BRANCH}` for PR #{PR_NUM} in `{REPO}`.
- Push all commits to branch `{BRANCH}` only. Do NOT create new PRs or new branches.
- If the branch already has commits, build on top of them — do not force-push or rebase.

## First: resolve merge conflicts

Before doing anything else, check if the branch is behind main:

```bash
git fetch origin main
git log HEAD..origin/main --oneline
```

If there are new commits on main, merge them in:

```bash
git merge origin/main
```

If there are conflicts, resolve them manually — read the conflicting files, understand both sides, keep the correct combination, then commit the merge. Push the merge commit before proceeding to implementation. This ensures the PR stays mergeable.

## Second: address existing review feedback

Before starting new work, check for existing review comments on this PR:

```bash
gh pr view {PR_NUM} --repo {REPO} --json comments --jq '.comments[].body'
```

If there are previous self-review summaries or review feedback comments:
1. Read each one carefully — these are findings from prior review cycles
2. Address any `REQUEST_CHANGES` items or actionable notes **before** doing anything else
3. Push fixes for each addressed finding
4. Do NOT run a new self-review just to repeat what was already found — only run self-review after you've made new changes

If the branch already has complete implementation and tests pass, and the only issue was merge conflicts (which you just resolved above), push the merge commit and stop. Do not re-run self-review on unchanged code.

## Third: scan for related issues

Before starting implementation, check if the structural fix you're about to make naturally addresses other open issues. This is the Raptor 1 → Raptor 3 principle: a well-designed fix to the underlying disease often cures multiple symptoms at once.

### 1. List open issues

```bash
gh issue list --repo {REPO} --state open --json number,title --limit 100
```

### 2. Identify candidates

Read the issue titles against your PR description. Look for issues that share the same:
- Files or modules you're already modifying
- Root cause or structural disease your PR addresses
- Code paths your fix already touches

Ignore issues that are clearly unrelated or would require work outside your PR's scope.

### 3. Read candidate bodies

For each promising title (typically 1-3, never more than 5):

```bash
gh issue view <N> --repo {REPO} --json body -q .body
```

### 4. Decide: in-scope or not

An issue is in-scope **only if** your existing fix already addresses it or addressing it requires trivial additional work in code you're already changing. Concretely:
- **Yes**: "Error handling missing in parser" — and your PR is restructuring that parser's error paths
- **Yes**: "Config key X not documented" — and your PR is already modifying that config module
- **No**: "Add retry logic to HTTP client" — even if related, this is a separate piece of work
- **No**: Anything that would add a new dependency, a new module, or more than ~30 lines of code beyond what the PR already requires

When in doubt, leave it out. A clean PR that closes 2 issues is better than a sprawling one that half-fixes 5.

### 5. Bring in-scope issues into the PR

For each issue you're absorbing:

```bash
gh pr edit {PR_NUM} --repo {REPO} --body "$(gh pr view {PR_NUM} --repo {REPO} --json body -q .body)

Closes #<N>"
```

This appends `Closes #N` to the PR body so GitHub auto-closes the issue on merge. Note this in your implementation plan — the reviewer should see that you consciously expanded scope with justification.

## How to work

- The PR description above is your complete briefing. Trust it.
- Design for elegance — aim for Raptor 1 to Raptor 3 structural improvements, not incremental patches.
- If removing code fixes the problem better than adding code, remove code.
- Push commits as you go. Update the PR body with what you actually did.
- Update issue labels as you resolve them (`gh issue edit --add-label needs-review --remove-label in-progress`).
- Keep the original PR plan intact — add an **Implementation** section below it with what was done, any deviations, and why.
- Curate tests: add tests for what you change, improve tests you encounter, remove flaky ones.
- It's okay to do less. A clean PR addressing 2 issues well beats a sprawling one addressing 5 poorly.
- Boy Scout Rule: when you touch a file, leave it better than you found it.

## Review, fix, and merge

After you finish implementation and all tests pass, run a review→fix loop and then merge the PR yourself. You own the full lifecycle.

**Skip this section entirely if**: the PR already has a self-review comment and you made no new code changes (e.g., you only resolved merge conflicts or addressed existing feedback). In that case, proceed directly to the merge step.

### 1. Get your diff

```bash
git diff origin/main...HEAD
```

### 2. Launch 4 review agents in parallel

Send a **single message** with 4 Task tool calls so they run concurrently. Each agent receives:

```
You are reviewing changes made by a worker in repository {REPO}.
The worker was assigned PR #{PR_NUM}. Review ONLY the diff provided below.

<pr-assignment>
<the PR description / assignment from above>
</pr-assignment>

<diff>
<your full diff output>
</diff>
```

The 4 agents:

1. **Security reviewer** — Scan the diff for: secrets or tokens, injection risks (SQL, command, path traversal), unsafe deserialization, hardcoded credentials, new dependencies with known CVEs, permission/auth changes. Only flag issues actually present in the diff.

2. **Architecture reviewer** — Evaluate the diff for: module boundary violations, increased coupling between components, broken abstraction layers, API surface changes without migration path, pattern breaks vs. established conventions. Base this on what you can infer from the diff and the codebase.

3. **Correctness reviewer** — Check: logic errors, off-by-one bugs, unhandled error cases, race conditions, null/None handling, integer overflow, resource leaks, incorrect state transitions.

4. **Test adequacy reviewer** — Check: new code has corresponding tests, changed behavior has updated tests, edge cases are covered, test assertions are meaningful (not just "it doesn't crash"), integration paths are tested.

Each agent must end its response with exactly one verdict line:

```
VERDICT: APPROVE
VERDICT: APPROVE_WITH_NOTES
VERDICT: REQUEST_CHANGES
```

Followed by a brief explanation (2-3 sentences max).

### 3. Address findings (max 2 review cycles)

**Cycle 1**: For each agent that returned `REQUEST_CHANGES`:
- Fix the issue in code
- Run tests again
- Push the fix

For agents that returned `APPROVE_WITH_NOTES`:
- Address the notes if they're actionable, otherwise note them for the summary

If all agents returned `APPROVE` or `APPROVE_WITH_NOTES`, skip to step 4.

If any agent returned `REQUEST_CHANGES`, after fixing and pushing, re-run the 4 review agents on the updated diff. This is **cycle 2** — the final review cycle.

**Cycle 2**: If all agents now approve, proceed to step 4. If any agent still returns `REQUEST_CHANGES` after 2 cycles, post the summary (step 4) but do **NOT** merge — leave the PR open for human review.

**Hard limit**: Never run more than 2 review cycles. Two rounds of review→fix is enough. If issues persist, a human should look.

### 4. Post review summary

Post a single comment on the PR summarizing what the review found and what was done about it.

```bash
gh pr comment {PR_NUM} --repo {REPO} --body "## Self-review summary

### Security
<findings and resolution, or 'No issues'>

### Architecture
<findings and resolution, or 'No issues'>

### Correctness
<findings and resolution, or 'No issues'>

### Test adequacy
<findings and resolution, or 'No issues'>

### Review cycles
<1 or 2> cycle(s). <Final outcome: all approved / still has open findings>"
```

### 5. Merge the PR

**Only merge if all reviewers approved** (after at most 2 cycles). If any reviewer still has `REQUEST_CHANGES` after 2 cycles, skip this step — leave the PR open for human review.

```bash
gh pr merge {PR_NUM} --repo {REPO} --squash --delete-branch
```

If the merge fails (branch protection, merge conflicts, permissions), report the error in a PR comment and stop. Do NOT force-push or attempt workarounds.

### 6. Close related issues and clean up

After a successful merge:

1. **Close related issues** that aren't auto-closed by `Closes #N` keywords in the PR body:

```bash
# For each issue referenced in the PR that doesn't have a Closes keyword
gh issue close <N> --repo {REPO} --comment "Resolved by PR #{PR_NUM}"
```

2. **Label transition** on related issues:

```bash
gh issue edit <N> --repo {REPO} --remove-label in-progress
```

### 7. Post final summary

After merge and cleanup, post a final comment confirming everything is done:

```bash
gh pr comment {PR_NUM} --repo {REPO} --body "## Worker complete

PR merged via squash. Related issues closed.

Review cycles: <1 or 2>
All reviewers: <APPROVE or APPROVE_WITH_NOTES>"
```

This gives the host session visibility into what happened.

### 8. Exit immediately

After posting the final summary you are DONE. Do NOT run tests, linters, or any
other commands after the PR is merged. The merge is proof the work succeeded —
further local activity wastes compute and can hang the container on slow test
suites. Exit cleanly and let the host session take over.
