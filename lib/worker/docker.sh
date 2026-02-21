#!/usr/bin/env bash
# sipag — worker Docker container orchestration
#
# Runs issue workers and PR iteration workers inside isolated Docker containers.
# Prompt construction, label transitions, hook invocations, and log capture are
# all handled here; GitHub queries come from github.sh, dedup from dedup.sh.
#
# Depends on globals set by config.sh:
#   WORKER_WORK_LABEL, WORKER_IN_PROGRESS_LABEL, WORKER_TIMEOUT_CMD, WORKER_TIMEOUT,
#   WORKER_OAUTH_TOKEN, WORKER_API_KEY, WORKER_GH_TOKEN,
#   WORKER_IMAGE, WORKER_LOG_DIR, SIPAG_DIR,
#   WORKER_REPO_MODEL, WORKER_REPO_PROMPT_EXTRA

# shellcheck disable=SC2154  # Globals defined in config.sh, sourced by worker.sh

# Run a single issue in a Docker container
worker_run_issue() {
    local repo="$1"
    local issue_num="$2"
    local title body branch slug pr_body prompt task_id start_time

    # Mark as in-progress so the spec is locked from edits
    worker_transition_label "$repo" "$issue_num" "$WORKER_WORK_LABEL" "$WORKER_IN_PROGRESS_LABEL"

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

    # Load prompt from template and substitute placeholders
    local _tpl_title='{{TITLE}}' _tpl_body='{{BODY}}' _tpl_branch='{{BRANCH}}' _tpl_issue_num='{{ISSUE_NUM}}'
    prompt=$(<"${_SIPAG_WORKER_LIB}/prompts/worker-issue.md")
    prompt="${prompt//${_tpl_title}/${title}}"
    prompt="${prompt//${_tpl_body}/${body}}"
    prompt="${prompt//${_tpl_branch}/${branch}}"
    prompt="${prompt//${_tpl_issue_num}/${issue_num}}"

    # Append per-repo extra instructions from .sipag.toml [prompts] if present
    if [[ -n "${WORKER_REPO_PROMPT_EXTRA:-}" ]]; then
        prompt="${prompt}

Project-specific requirements (from .sipag.toml):
${WORKER_REPO_PROMPT_EXTRA}"
    fi

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
        -e SIPAG_MODEL="${WORKER_REPO_MODEL:-}" \
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
            if [ -n "$SIPAG_MODEL" ]; then
                claude --print --dangerously-skip-permissions --model "$SIPAG_MODEL" -p "$PROMPT"
            else
                claude --print --dangerously-skip-permissions -p "$PROMPT"
            fi \
                && { gh pr ready "$BRANCH" --repo "'"${repo}"'" || true; \
                     echo "[sipag] PR marked ready for review"; }
        ' > "${WORKER_LOG_DIR}/issue-${issue_num}.log" 2>&1

    local exit_code=$?
    local duration
    duration=$(( $(date +%s) - start_time ))

    if [[ $exit_code -eq 0 ]]; then
        # Success: remove in-progress (PR's "Closes #N" handles the rest)
        worker_transition_label "$repo" "$issue_num" "$WORKER_IN_PROGRESS_LABEL" ""
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
        worker_transition_label "$repo" "$issue_num" "$WORKER_IN_PROGRESS_LABEL" "$WORKER_WORK_LABEL"
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

    # Load prompt from template and substitute placeholders
    local issue_body_display="${issue_body:-<not found>}"
    local _tpl_pr_num='{{PR_NUM}}' _tpl_repo='{{REPO}}' _tpl_issue_body='{{ISSUE_BODY}}'
    local _tpl_pr_diff='{{PR_DIFF}}' _tpl_review='{{REVIEW_FEEDBACK}}' _tpl_branch='{{BRANCH}}'
    prompt=$(<"${_SIPAG_WORKER_LIB}/prompts/worker-iteration.md")
    prompt="${prompt//${_tpl_pr_num}/${pr_num}}"
    prompt="${prompt//${_tpl_repo}/${repo}}"
    prompt="${prompt//${_tpl_issue_body}/${issue_body_display}}"
    prompt="${prompt//${_tpl_pr_diff}/${pr_diff}}"
    prompt="${prompt//${_tpl_review}/${review_feedback}}"
    prompt="${prompt//${_tpl_branch}/${branch_name}}"

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
        -e SIPAG_MODEL="${WORKER_REPO_MODEL:-}" \
        -e PROMPT \
        -e BRANCH \
        "$WORKER_IMAGE" \
        bash -c '
            git clone https://github.com/'"${repo}"'.git /work && cd /work
            git config user.name "sipag"
            git config user.email "sipag@localhost"
            git remote set-url origin "https://x-access-token:${GH_TOKEN}@github.com/'"${repo}"'.git"
            git checkout "$BRANCH"
            if [ -n "$SIPAG_MODEL" ]; then
                claude --print --dangerously-skip-permissions --model "$SIPAG_MODEL" -p "$PROMPT"
            else
                claude --print --dangerously-skip-permissions -p "$PROMPT"
            fi
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
