#!/usr/bin/env bash
# sipag — worker Docker container orchestration
#
# Runs issue workers and PR iteration workers inside isolated Docker containers.
# Prompt construction, label transitions, hook invocations, and log capture are
# all handled here; GitHub queries come from github.sh, dedup from dedup.sh.
#
# Depends on globals set by config.sh:
#   WORKER_WORK_LABEL, WORKER_TIMEOUT_CMD, WORKER_TIMEOUT,
#   WORKER_OAUTH_TOKEN, WORKER_API_KEY, WORKER_GH_TOKEN,
#   WORKER_IMAGE, WORKER_LOG_DIR, SIPAG_DIR

# shellcheck disable=SC2154  # Globals defined in config.sh, sourced by worker.sh

# Run a single issue in a Docker container
worker_run_issue() {
    local repo="$1"
    local issue_num="$2"
    local title body branch slug pr_body prompt task_id start_time

    # Mark as in-progress so the spec is locked from edits
    worker_transition_label "$repo" "$issue_num" "$WORKER_WORK_LABEL" "in-progress"

    # Fetch the spec fresh right before starting (minimizes stale-spec window)
    title=$(gh issue view "$issue_num" --repo "$repo" --json title -q '.title')
    body=$(gh issue view "$issue_num" --repo "$repo" --json body -q '.body')

    echo "[#${issue_num}] Starting: $title"

    # Generate branch name and draft PR body before entering container
    slug=$(worker_slugify "$title")
    branch="sipag/issue-${issue_num}-${slug}"
    task_id="$(date +%Y%m%d)-${slug}"

    pr_body="Closes #${issue_num}

${body}

---
*This PR was opened by a sipag worker. Commits will appear as work progresses.*"

    prompt="You are working on the repository at /work.

Your task:
${title}

${body}

Instructions:
- You are on branch ${branch} — do NOT create a new branch
- A draft PR is already open for this branch — do not open another one
- Implement the changes
- Run \`make dev\` (fmt + clippy + test) before committing to validate your changes
- Run any existing tests and make sure they pass
- Commit your changes with a clear commit message and push to origin
- The PR will be marked ready for review automatically when you finish
- The PR should close issue #${issue_num}"

    # Hook: worker started
    export SIPAG_EVENT="worker.started"
    export SIPAG_REPO="$repo"
    export SIPAG_ISSUE="$issue_num"
    export SIPAG_ISSUE_TITLE="$title"
    export SIPAG_TASK_ID="$task_id"
    sipag_run_hook "on-worker-started"

    start_time=$(date +%s)
    PROMPT="$prompt" BRANCH="$branch" ISSUE_TITLE="$title" PR_BODY="$pr_body" \
        ${WORKER_TIMEOUT_CMD:+$WORKER_TIMEOUT_CMD $WORKER_TIMEOUT} docker run --rm \
        -e CLAUDE_CODE_OAUTH_TOKEN="${WORKER_OAUTH_TOKEN}" \
        -e ANTHROPIC_API_KEY="${WORKER_API_KEY}" \
        -e GH_TOKEN="$WORKER_GH_TOKEN" \
        -e PROMPT \
        -e BRANCH \
        -e ISSUE_TITLE \
        -e PR_BODY \
        "$WORKER_IMAGE" \
        bash -c '
            git clone "https://github.com/'"${repo}"'.git" /work && cd /work
            git config user.name "sipag"
            git config user.email "sipag@localhost"
            git remote set-url origin "https://x-access-token:${GH_TOKEN}@github.com/'"${repo}"'.git"
            git checkout -b "$BRANCH"
            git push -u origin "$BRANCH"
            gh pr create --repo "'"${repo}"'" \
                --title "$ISSUE_TITLE" \
                --body "$PR_BODY" \
                --draft \
                --head "$BRANCH"
            echo "[sipag] Draft PR opened: branch=$BRANCH issue='"${issue_num}"'"
            claude --print --dangerously-skip-permissions -p "$PROMPT" \
                && { gh pr ready "$BRANCH" --repo "'"${repo}"'" || true; \
                     echo "[sipag] PR marked ready for review"; }
        ' > "${WORKER_LOG_DIR}/issue-${issue_num}.log" 2>&1

    local exit_code=$?
    local duration
    duration=$(( $(date +%s) - start_time ))

    if [[ $exit_code -eq 0 ]]; then
        # Success: remove in-progress (PR's "Closes #N" handles the rest)
        worker_transition_label "$repo" "$issue_num" "in-progress" ""
        echo "[#${issue_num}] DONE: $title"

        # Look up the PR opened by the worker
        local pr_num pr_url
        pr_num=$(gh pr list --repo "$repo" --head "$branch" --json number -q '.[0].number' 2>/dev/null || true)
        pr_url=$(gh pr list --repo "$repo" --head "$branch" --json url -q '.[0].url' 2>/dev/null || true)

        # Hook: worker completed
        export SIPAG_EVENT="worker.completed"
        export SIPAG_PR_NUM="${pr_num:-}"
        export SIPAG_PR_URL="${pr_url:-}"
        export SIPAG_DURATION="$duration"
        sipag_run_hook "on-worker-completed"
    else
        # Failure: move back to approved for retry (draft PR stays open showing progress)
        worker_transition_label "$repo" "$issue_num" "in-progress" "$WORKER_WORK_LABEL"
        echo "[#${issue_num}] FAILED (exit ${exit_code}): $title — returned to ${WORKER_WORK_LABEL}"

        # Hook: worker failed
        export SIPAG_EVENT="worker.failed"
        export SIPAG_EXIT_CODE="$exit_code"
        export SIPAG_LOG_PATH="${WORKER_LOG_DIR}/issue-${issue_num}.log"
        sipag_run_hook "on-worker-failed"
    fi
}

# Run a PR iteration: checkout existing branch, read review feedback, push fixes
worker_run_pr_iteration() {
    local repo="$1"
    local pr_num="$2"
    local title branch_name issue_num issue_body review_feedback pr_diff prompt

    worker_pr_mark_running "$pr_num"

    title=$(gh pr view "$pr_num" --repo "$repo" --json title -q '.title' 2>/dev/null)
    branch_name=$(gh pr view "$pr_num" --repo "$repo" --json headRefName -q '.headRefName' 2>/dev/null)

    echo "[PR #${pr_num}] Iterating: $title (branch: $branch_name)"

    # Extract linked issue number from PR body (e.g. "Closes #42")
    issue_num=$(gh pr view "$pr_num" --repo "$repo" --json body -q '.body' 2>/dev/null \
        | grep -oiE '(closes|fixes|resolves) #[0-9]+' | grep -oE '[0-9]+' | head -1 || true)

    issue_body=""
    if [[ -n "$issue_num" ]]; then
        issue_body=$(gh issue view "$issue_num" --repo "$repo" --json body -q '.body' 2>/dev/null || true)
    fi

    # Collect review feedback: CHANGES_REQUESTED reviews + all PR comments
    review_feedback=$(gh pr view "$pr_num" --repo "$repo" --json reviews,comments \
        --jq '([.reviews[] | select(.state == "CHANGES_REQUESTED") | "Review by \(.author.login):\n\(.body)"] +
               [.comments[] | "Comment by \(.author.login):\n\(.body)"]) | join("\n---\n")' 2>/dev/null || true)

    # Inline code review comments (line-level feedback on the diff, via REST API)
    local inline_comments
    inline_comments=$(gh api "repos/${repo}/pulls/${pr_num}/comments" \
        --jq '[.[] | "Inline comment on \(.path) line \(.line // "?") by \(.user.login):\n\(.body)"] | join("\n---\n")' 2>/dev/null || true)
    if [[ -n "$inline_comments" ]]; then
        [[ -n "$review_feedback" ]] && review_feedback+=$'\n---\n'
        review_feedback+="$inline_comments"
    fi

    # Capture current diff (capped to avoid overwhelming the prompt)
    pr_diff=$(gh pr diff "$pr_num" --repo "$repo" 2>/dev/null | head -c 50000 || true)

    prompt="You are iterating on PR #${pr_num} in ${repo}.

Original issue:
${issue_body:-<not found>}

Current PR diff:
${pr_diff}

Review feedback:
${review_feedback}

Instructions:
- You are on branch ${branch_name} which already has work in progress
- Read the review feedback carefully and address every point raised
- Make targeted changes that address the feedback
- Do NOT rewrite the PR from scratch — make surgical fixes
- Run \`make dev\` (fmt + clippy + test) before committing to validate your changes
- Commit with a message that references the feedback (do NOT amend existing commits)
- Push to the same branch (git push origin ${branch_name}) — do NOT force push"

    # Hook: PR iteration started
    export SIPAG_EVENT="pr-iteration.started"
    export SIPAG_REPO="$repo"
    export SIPAG_PR_NUM="$pr_num"
    export SIPAG_ISSUE="${issue_num:-}"
    export SIPAG_ISSUE_TITLE="$title"
    sipag_run_hook "on-pr-iteration-started"

    PROMPT="$prompt" BRANCH="$branch_name" \
        ${WORKER_TIMEOUT_CMD:+$WORKER_TIMEOUT_CMD $WORKER_TIMEOUT} docker run --rm \
        -e CLAUDE_CODE_OAUTH_TOKEN="${WORKER_OAUTH_TOKEN}" \
        -e ANTHROPIC_API_KEY="${WORKER_API_KEY}" \
        -e GH_TOKEN="$WORKER_GH_TOKEN" \
        -e PROMPT \
        -e BRANCH \
        "$WORKER_IMAGE" \
        bash -c '
            git clone https://github.com/'"${repo}"'.git /work && cd /work
            git config user.name "sipag"
            git config user.email "sipag@localhost"
            git remote set-url origin "https://x-access-token:${GH_TOKEN}@github.com/'"${repo}"'.git"
            git checkout "$BRANCH"
            claude --print --dangerously-skip-permissions -p "$PROMPT"
        ' > "${WORKER_LOG_DIR}/pr-${pr_num}-iter.log" 2>&1

    local exit_code=$?
    worker_pr_mark_done "$pr_num"

    # Hook: PR iteration done
    export SIPAG_EVENT="pr-iteration.done"
    export SIPAG_EXIT_CODE="$exit_code"
    sipag_run_hook "on-pr-iteration-done"

    if [[ $exit_code -eq 0 ]]; then
        echo "[PR #${pr_num}] DONE iterating: $title"
    else
        echo "[PR #${pr_num}] FAILED iteration (exit ${exit_code}): $title"
    fi
}
