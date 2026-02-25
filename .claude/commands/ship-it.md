Commit, branch, push, create a PR, review-fix until clean, and merge. End-to-end ship.

## Instructions

You are orchestrating the full ship workflow: commit → branch → push → PR → review-fix loop → merge. The argument $ARGUMENTS is optional context (e.g., a PR title hint).

### Step 1: Prepare the branch

Check the current branch with `git branch --show-current`.

**If on `main`:**

1. Check `git status` and `git diff` for uncommitted changes (staged, unstaged, and untracked).
2. If there are changes:
   - Analyze the changes to derive a short kebab-case branch name (e.g., `feat/generative-init`, `fix/heartbeat-race`).
   - Create and switch to a feature branch: `git checkout -b <branch-name>`
   - Stage all relevant changes: `git add <specific files>` (avoid secrets, .env, credentials)
   - Commit with a descriptive message summarizing the changes.
3. If there are no changes, stop and tell the user: "Nothing to ship — no uncommitted changes and already on main."

**If on a feature branch:**

1. Check `git status` for uncommitted changes.
2. If there are changes:
   - Stage all relevant changes: `git add <specific files>`
   - Commit with a descriptive message.
3. If there are no changes, that's fine — continue with whatever is already committed on the branch.

### Step 2: Push and create the PR

1. Push the branch to origin:
   ```
   git push -u origin $(git branch --show-current)
   ```

2. Check if a PR already exists for this branch:
   ```
   gh pr list --head $(git branch --show-current) --json number,url -q '.[0]'
   ```

3. If no PR exists, create one:
   - Gather the commit log for the PR body: `git log main..HEAD --oneline`
   - Create the PR:
     ```
     gh pr create --title "<descriptive title>" --body "$(cat <<'EOF'
     ## Summary
     <1-3 bullet points summarizing the changes based on commit history>

     ## Test plan
     - [ ] `make dev` passes (fmt + clippy + test)
     - [ ] Manual verification of changed functionality
     EOF
     )"
     ```
   - If `$ARGUMENTS` is provided and looks like a title, use it as the PR title.
   - Otherwise, derive the title from the branch name and commits.

4. Store the PR number for subsequent steps.

### Step 3: Run the review-fix loop

#### Step 3a: Gather the diff

Fetch the diff: `gh pr diff <PR_NUMBER>`
Fetch the PR description: `gh pr view <PR_NUMBER>`

#### Step 3b: Identify changed files

List all files changed in the diff. Read the full content of each changed file (not just the diff hunks) so reviewers have complete context.

#### Step 3c: Launch parallel review agents

Launch ALL of the following review agents in parallel using the Task tool. Each agent should receive:
1. The full diff
2. The list of changed files
3. The full content of each changed file

**Agents to launch:**

1. **Security Review** (subagent_type: "security-reviewer")
   - Prompt: Review these changes as a security reviewer. Perform STRIDE threat modeling and OWASP checks. Focus on: token handling (OAuth, API key, GH token resolution in `auth.rs`), Docker container escapes, command injection in shell exec paths, credential exposure in state files or logs, path traversal in state file operations. Here is the diff: [include diff]. Here are the full file contents: [include file contents]. Respond with LGTM if no issues, otherwise list issues as `[severity: high|medium|low] file:line — description`.

2. **Architecture Review** (subagent_type: "architecture-reviewer")
   - Prompt: Review these changes as an architecture reviewer. Check: Rust crate boundaries (sipag-core vs sipag vs tui vs sipag-worker), bash script organization, worker prompt construction, config resolution order (env → file → default), state file format changes, template embedding patterns. Here is the diff: [include diff]. Here are the full file contents: [include file contents]. Respond with LGTM if no issues, otherwise list issues as `[severity: high|medium|low] file:line — description`.

3. **Correctness Review** (subagent_type: "correctness-reviewer")
   - Prompt: Review these changes for correctness. Check: worker lifecycle edge cases (starting → working → finished/failed), race conditions in parallel workers writing state files, GitHub API error handling, heartbeat-based liveness detection, atomic state file writes, Docker container cleanup. Here is the diff: [include diff]. Here are the full file contents: [include file contents]. Respond with LGTM if no issues, otherwise list issues as `[severity: high|medium|low] file:line — description`.

4. **Test Adequacy Review** (subagent_type: "general-purpose")
   - Prompt: Review these changes for test adequacy. Check: new code has corresponding tests, changed behavior has updated tests, edge cases are covered (especially around state transitions, config parsing, and error paths), test assertions are meaningful. The project uses `cargo test --workspace` for testing. Here is the diff: [include diff]. Here are the full file contents: [include file contents]. Respond with LGTM if no issues, otherwise list issues as `[severity: high|medium|low] file:line — description`.

Each agent must end its response with exactly one verdict line:

```
VERDICT: APPROVE
VERDICT: APPROVE_WITH_NOTES
VERDICT: REQUEST_CHANGES
```

#### Step 3d: Compile and post the review

Once all agents complete, compile a unified review report and post it as a comment on the PR using `gh pr comment`:

```
## Review Summary for PR #<N>

### Security
<verdict> — <key findings or "No issues">

### Architecture
<verdict> — <key findings or "No issues">

### Correctness
<verdict> — <key findings or "No issues">

### Test Adequacy
<verdict> — <key findings or "No issues">

### Overall
<APPROVE / APPROVE_WITH_NOTES / REQUEST_CHANGES>
<1-2 sentence summary>

### Issues by Severity
#### High
- [list all high severity issues across all reviewers, if any]

#### Medium
- [list all medium severity issues across all reviewers, if any]

#### Low
- [list all low severity issues across all reviewers, if any]
```

Use a HEREDOC to pass the review body:
```
gh pr comment <PR_NUMBER> --body "$(cat <<'REVIEW_EOF'
<compiled review markdown>
REVIEW_EOF
)"
```

If all reviewers say LGTM, the verdict is APPROVE.
If any reviewer has HIGH severity issues, the verdict is REQUEST CHANGES.
Otherwise, the verdict is APPROVE_WITH_NOTES.

#### Step 3e: Fix all issues

If the verdict is APPROVE (all LGTM), skip to Step 4.

Otherwise, fix every issue found by the reviewers — high, medium, and low. For each issue:
1. Read the relevant file(s) to understand the context
2. Make the fix using Edit/Write tools
3. Keep fixes minimal and focused — don't refactor beyond what the issue requires

After fixing all issues, run validation (`make dev`) to make sure nothing is broken. If checks fail, fix them before proceeding.

Commit all fixes in a single commit with a message summarizing what was addressed:
```
fix: address review findings — [brief list of what was fixed]
```

Push the commit to the PR branch.

#### Step 3f: Re-review (loop)

Go back to Step 3a: gather the fresh diff, identify changed files, launch all 4 review agents again in parallel, compile the new review, and post it as a new comment on the PR.

**Keep looping Steps 3a–3f until the verdict is APPROVE** (all agents return LGTM or only have findings that are intentional/acknowledged).

To prevent infinite loops: if the same issue appears in 3 consecutive review rounds, stop the loop, post a comment explaining the unresolved issue, and ask the user for guidance.

### Step 4: Merge the PR

Once the review loop completes with APPROVE:

1. Post a final comment:
   ```
   gh pr comment <PR_NUMBER> --body "All review agents report LGTM. Merging."
   ```

2. Merge the PR using squash merge to keep history clean:
   ```
   gh pr merge <PR_NUMBER> --squash --delete-branch
   ```

3. Switch back to main, pull, and delete the local branch:
   ```
   BRANCH=$(git branch --show-current)
   git checkout main && git pull && git branch -d "$BRANCH"
   ```

4. Tell the user the PR has been merged and the branch cleaned up (both remote and local). Include the PR URL in the final message.
