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

## How to work

- The PR description above is your complete briefing. Trust it.
- Design for elegance — aim for Raptor 1 to Raptor 3 structural improvements, not incremental patches.
- If removing code fixes the problem better than adding code, remove code.
- If your changes accidentally resolve issues not in the plan, add `Closes #N` to the PR body.
- Push commits as you go. Update the PR body with what you actually did.
- Update issue labels as you resolve them (`gh issue edit --add-label needs-review --remove-label in-progress`).
- Keep the original PR plan intact — add an **Implementation** section below it with what was done, any deviations, and why.
- Curate tests: add tests for what you change, improve tests you encounter, remove flaky ones.
- It's okay to do less. A clean PR addressing 2 issues well beats a sprawling one addressing 5 poorly.
- Boy Scout Rule: when you touch a file, leave it better than you found it.

## Self-review

After you finish implementation and all tests pass, run a self-review before declaring done. This catches issues while you can still fix them.

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

### 3. Address findings

For each agent that returned `REQUEST_CHANGES`:
- Fix the issue in code
- Run tests again
- Push the fix

For agents that returned `APPROVE_WITH_NOTES`:
- Address the notes if they're actionable, otherwise note them for the PR comment

### 4. Post review summary on the PR

After addressing all findings, post a comment on the PR summarizing what the self-review found and what was done about it:

```bash
gh pr comment {PR_NUM} --repo {REPO} --body "## Self-review summary

### Security
<findings and resolution, or 'No issues'>

### Architecture
<findings and resolution, or 'No issues'>

### Correctness
<findings and resolution, or 'No issues'>

### Test adequacy
<findings and resolution, or 'No issues'>"
```

This gives the reviewer (human or main session) visibility into what was caught and fixed during implementation.
