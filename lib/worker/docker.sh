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

# Write minimal "enqueued" worker state JSON to ~/.sipag/workers/ before
# the container starts. This guarantees the issue is never silently dropped
# if the process crashes between issue discovery and container launch.
# $1: repo slug (OWNER--REPO), $2: issue_num
_worker_write_enqueued_state() {
    local repo_slug="$1" issue_num="$2"
    local state_file="${SIPAG_DIR}/workers/${repo_slug}--${issue_num}.json"
    local container_name="sipag-issue-${issue_num}"
    local now
    now=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
    mkdir -p "${SIPAG_DIR}/workers"
    # shellcheck disable=SC2016  # jq variables are not shell variables
    jq -n \
        --arg repo "${repo_slug/--//}" \
        --argjson issue_num "$issue_num" \
        --arg container_name "$container_name" \
        --arg started_at "$now" \
        '{
            repo: $repo,
            issue_num: $issue_num,
            issue_title: "",
            branch: "",
            container_name: $container_name,
            pr_num: null,
            pr_url: null,
            status: "enqueued",
            started_at: $started_at,
            ended_at: null,
            duration_s: null,
            exit_code: null,
            log_path: null
        }' > "$state_file"
}

# Write initial worker state JSON to ~/.sipag/workers/
# $1: repo slug (OWNER--REPO), $2: issue_num, $3: issue_title,
# $4: branch, $5: container_name, $6: started_at, $7: log_path
_worker_write_state() {
    local repo_slug="$1" issue_num="$2" issue_title="$3"
    local branch="$4" container_name="$5" started_at="$6" log_path="$7"
    local state_file="${SIPAG_DIR}/workers/${repo_slug}--${issue_num}.json"
    # shellcheck disable=SC2016  # jq variables ($repo etc.) are not shell variables
    jq -n \
        --arg repo "${repo_slug/--//}" \
        --argjson issue_num "$issue_num" \
        --arg issue_title "$issue_title" \
        --arg branch "$branch" \
        --arg container_name "$container_name" \
        --arg started_at "$started_at" \
        --arg log_path "$log_path" \
        '{
            repo: $repo,
            issue_num: $issue_num,
            issue_title: $issue_title,
            branch: $branch,
            container_name: $container_name,
            pr_num: null,
            pr_url: null,
            status: "running",
            started_at: $started_at,
            ended_at: null,
            duration_s: null,
            exit_code: null,
            log_path: $log_path
        }' > "$state_file"
}

# Update worker state JSON on completion
# $1: repo_slug, $2: issue_num, $3: status (done|failed), $4: exit_code,
# $5: ended_at, $6: duration_s, $7: pr_num (optional), $8: pr_url (optional)
_worker_update_state() {
    local repo_slug="$1" issue_num="$2" status="$3" exit_code="$4"
    local ended_at="$5" duration_s="$6" pr_num="${7:-}" pr_url="${8:-}"
    local state_file="${SIPAG_DIR}/workers/${repo_slug}--${issue_num}.json"
    [[ -f "$state_file" ]] || return 0
    local tmp
    tmp=$(mktemp)
    jq \
        --arg status "$status" \
        --argjson exit_code "$exit_code" \
        --arg ended_at "$ended_at" \
        --argjson duration_s "$duration_s" \
        --arg pr_num "$pr_num" \
        --arg pr_url "$pr_url" \
        '.status = $status |
         .exit_code = $exit_code |
         .ended_at = $ended_at |
         .duration_s = $duration_s |
         .pr_num = (if $pr_num == "" then null else ($pr_num | tonumber) end) |
         .pr_url = (if $pr_url == "" then null else $pr_url end)' \
        "$state_file" > "$tmp" && mv "$tmp" "$state_file"
}

# Finalize a state file for a container that is no longer running.
# Checks for PRs, transitions labels, and updates state to done or failed.
# $1: repo, $2: issue_num, $3: branch, $4: repo_slug
_worker_finalize_gone_container() {
    local repo="$1" issue_num="$2" branch="$3" repo_slug="$4"
    local ended_at
    ended_at=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

    local pr_num pr_url
    pr_num=$(gh pr list --repo "$repo" --head "$branch" --state all \
        --json number -q '.[0].number' 2>/dev/null || true)

    if [[ -n "$pr_num" ]]; then
        pr_url=$(gh pr list --repo "$repo" --head "$branch" --state all \
            --json url -q '.[0].url' 2>/dev/null || true)
        worker_transition_label "$repo" "$issue_num" "in-progress" ""
        _worker_update_state "$repo_slug" "$issue_num" "done" "0" \
            "$ended_at" "0" "${pr_num:-}" "${pr_url:-}"
        echo "[$(date +%H:%M:%S)] Finalized: #${issue_num} → done (PR #${pr_num})"
    else
        worker_transition_label "$repo" "$issue_num" "in-progress" "$WORKER_WORK_LABEL"
        _worker_update_state "$repo_slug" "$issue_num" "failed" "1" "$ended_at" "0"
        echo "[$(date +%H:%M:%S)] Finalized: #${issue_num} → failed (no PR found)"
    fi
}

# Prune terminal state files (status: done or failed) older than WORKER_STATE_MAX_AGE_DAYS.
#
# Called once at startup before worker_recover() so stale state from old/irrelevant
# repos does not accumulate forever. Only terminal states (done/failed) are pruned;
# active states (enqueued/running/recovering) are never deleted automatically.
#
# Age is determined by the file's mtime. The threshold defaults to 7 days and is
# configurable via SIPAG_STATE_MAX_AGE_DAYS or the state_max_age_days= config key.
worker_prune_state_files() {
    local workers_dir="${SIPAG_DIR}/workers"
    [[ -d "$workers_dir" ]] || return 0

    local max_age_days="${WORKER_STATE_MAX_AGE_DAYS:-7}"
    local pruned=0

    for state_file in "${workers_dir}"/*.json; do
        [[ -f "$state_file" ]] || continue

        local status
        status=$(jq -r '.status' "$state_file" 2>/dev/null || true)
        # Only prune terminal states — never touch active entries
        [[ "$status" == "done" || "$status" == "failed" ]] || continue

        # Check file age using find -mtime; +N means "older than N days"
        if [[ -n "$(find "$state_file" -mtime "+${max_age_days}" 2>/dev/null)" ]]; then
            rm -f "$state_file"
            pruned=$(( pruned + 1 ))
        fi
    done

    [[ $pruned -gt 0 ]] && echo "[$(date +%H:%M:%S)] Pruned ${pruned} terminal state file(s) older than ${max_age_days} days"
    return 0
}

# Recover orphaned worker state files on startup.
#
# Scans ~/.sipag/workers/*.json for entries with status "running" or
# "recovering" (stale from a previous buggy recovery). These are left over
# when the worker process dies while containers are still active.
#
# For each orphaned entry:
#   - Container still running: leave state as "running" — worker_finalize_exited()
#     will catch it when the container eventually exits.
#   - Container gone (exited and self-cleaned via --rm):
#       PR exists (open or merged) → state → done, remove in-progress label
#       No PR found              → state → failed, restore WORKER_WORK_LABEL
#
# No background subshells are spawned. State finalization is synchronous for
# gone containers and deferred to worker_finalize_exited() for live ones.
worker_recover() {
    local workers_dir="${SIPAG_DIR}/workers"
    [[ -d "$workers_dir" ]] || return 0

    # Prune old terminal state files first so they don't pollute recovery output
    worker_prune_state_files

    local adopted=0 finalized=0

    for state_file in "${workers_dir}"/*.json; do
        [[ -f "$state_file" ]] || continue

        local status
        status=$(jq -r '.status' "$state_file" 2>/dev/null || true)
        # Handle "running" (normal), "recovering" (stale from old code), and
        # "enqueued" (identified but container never started due to crash)
        [[ "$status" == "running" || "$status" == "recovering" || "$status" == "enqueued" ]] || continue

        local repo issue_num branch container_name
        repo=$(jq -r '.repo' "$state_file")
        issue_num=$(jq -r '.issue_num' "$state_file")
        branch=$(jq -r '.branch' "$state_file")
        container_name=$(jq -r '.container_name' "$state_file")

        local repo_slug="${repo//\//--}"

        # Enqueued workers have no container; the process crashed before the
        # container could start. Restore the work label and mark as failed so
        # the next poll cycle re-dispatches the issue.
        if [[ "$status" == "enqueued" ]]; then
            echo "[$(date +%H:%M:%S)] Recovery: #${issue_num} was enqueued but container never started — returning to ${WORKER_WORK_LABEL}"
            worker_transition_label "$repo" "$issue_num" "in-progress" "$WORKER_WORK_LABEL"
            local tmp_enq now_enq
            now_enq=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
            tmp_enq=$(mktemp)
            jq --arg status "failed" --arg ended_at "$now_enq" \
               '.status = $status | .ended_at = $ended_at' \
               "$state_file" > "$tmp_enq" && mv "$tmp_enq" "$state_file"
            finalized=$(( finalized + 1 ))
            continue
        fi

        if [[ -n "$(docker ps --filter "name=^${container_name}$" --format '{{.Names}}' 2>/dev/null)" ]]; then
            # Container still running: leave as "running", finalize_exited will catch it
            echo "[$(date +%H:%M:%S)] Recovery: container ${container_name} still running (#${issue_num}) — will finalize when it exits"

            # If stuck at "recovering" from old code, reset to "running"
            if [[ "$status" == "recovering" ]]; then
                local tmp
                tmp=$(mktemp)
                jq '.status = "running"' "$state_file" > "$tmp" && mv "$tmp" "$state_file"
            fi

            adopted=$(( adopted + 1 ))
        else
            # Container gone: finalize synchronously
            echo "[$(date +%H:%M:%S)] Recovery: container ${container_name} gone, checking PR for #${issue_num}"
            _worker_finalize_gone_container "$repo" "$issue_num" "$branch" "$repo_slug"
            finalized=$(( finalized + 1 ))
        fi
    done

    local total=$(( adopted + finalized ))
    [[ $total -gt 0 ]] && echo "[$(date +%H:%M:%S)] Recovery complete: ${adopted} adopted, ${finalized} finalized"
    return 0
}

# Check all "running" state files and finalize any whose containers have exited.
#
# Called at the top of each poll cycle so that containers adopted by
# worker_recover() (or left over from a killed worker) get their state
# files updated without relying on fragile background subshells.
worker_finalize_exited() {
    local workers_dir="${SIPAG_DIR}/workers"
    [[ -d "$workers_dir" ]] || return 0

    for state_file in "${workers_dir}"/*.json; do
        [[ -f "$state_file" ]] || continue

        local status
        status=$(jq -r '.status' "$state_file" 2>/dev/null || true)
        [[ "$status" == "running" || "$status" == "recovering" || "$status" == "enqueued" ]] || continue

        local repo issue_num branch container_name
        repo=$(jq -r '.repo' "$state_file")
        issue_num=$(jq -r '.issue_num' "$state_file")
        branch=$(jq -r '.branch' "$state_file")
        container_name=$(jq -r '.container_name' "$state_file")

        local repo_slug="${repo//\//--}"

        # Enqueued workers have no container (written before docker run was called).
        # If still enqueued at finalization time, the worker subprocess died before
        # transitioning to "running". Restore the work label and mark as failed.
        if [[ "$status" == "enqueued" ]]; then
            echo "[$(date +%H:%M:%S)] Enqueued worker #${issue_num} — container never started, returning to ${WORKER_WORK_LABEL}"
            worker_transition_label "$repo" "$issue_num" "in-progress" "$WORKER_WORK_LABEL"
            local tmp_enq now_enq
            now_enq=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
            tmp_enq=$(mktemp)
            jq --arg status "failed" --arg ended_at "$now_enq" \
               '.status = $status | .ended_at = $ended_at' \
               "$state_file" > "$tmp_enq" && mv "$tmp_enq" "$state_file"
            continue
        fi

        # Container still alive → skip, it's still working
        if [[ -n "$(docker ps --filter "name=^${container_name}$" --format '{{.Names}}' 2>/dev/null)" ]]; then
            continue
        fi

        # Container gone → finalize
        echo "[$(date +%H:%M:%S)] Container ${container_name} exited, finalizing #${issue_num}"
        _worker_finalize_gone_container "$repo" "$issue_num" "$branch" "$repo_slug"
    done
}

# Run a single issue in a Docker container
worker_run_issue() {
    local repo="$1"
    local issue_num="$2"
    local title body branch slug pr_body prompt task_id start_time

    # Record the issue as enqueued immediately — before the label transition —
    # so that a crash between discovery and container start leaves a recoverable
    # state file instead of silently dropping the issue.
    _worker_write_enqueued_state "$WORKER_REPO_SLUG" "$issue_num"

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

    # Load prompt from template and substitute placeholders
    local _tpl_title='{{TITLE}}' _tpl_body='{{BODY}}' _tpl_branch='{{BRANCH}}' _tpl_issue_num='{{ISSUE_NUM}}'
    prompt=$(<"${_SIPAG_WORKER_LIB}/prompts/worker-issue.md")
    prompt="${prompt//${_tpl_title}/${title}}"
    prompt="${prompt//${_tpl_body}/${body}}"
    prompt="${prompt//${_tpl_branch}/${branch}}"
    prompt="${prompt//${_tpl_issue_num}/${issue_num}}"

    # Hook: worker started
    export SIPAG_EVENT="worker.started"
    export SIPAG_REPO="$repo"
    export SIPAG_ISSUE="$issue_num"
    export SIPAG_ISSUE_TITLE="$title"
    export SIPAG_TASK_ID="$task_id"
    sipag_run_hook "on-worker-started"

    local container_name="sipag-issue-${issue_num}"
    local log_path="${WORKER_LOG_DIR}/${WORKER_REPO_SLUG}--${issue_num}.log"
    local started_at
    started_at=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
    start_time=$(date +%s)

    # Write initial worker state file
    _worker_write_state \
        "$WORKER_REPO_SLUG" "$issue_num" "$title" \
        "$branch" "$container_name" "$started_at" "$log_path"

    PROMPT="$prompt" BRANCH="$branch" ISSUE_TITLE="$title" PR_BODY="$pr_body" \
        ${WORKER_TIMEOUT_CMD:+$WORKER_TIMEOUT_CMD $WORKER_TIMEOUT} docker run --rm \
        --name "$container_name" \
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
            # Attempt early draft PR. GitHub rejects this when the branch has no commits
            # yet ("No commits between main and BRANCH"). Capture the failure and defer —
            # the PR will be created (or retried) after Claude pushes its work.
            if gh pr create --repo "'"${repo}"'" \
                    --title "$ISSUE_TITLE" \
                    --body "$PR_BODY" \
                    --draft \
                    --head "$BRANCH" 2>/tmp/sipag-pr-err.log; then
                echo "[sipag] Draft PR opened: branch=$BRANCH issue='"${issue_num}"'"
            else
                echo "[sipag] Draft PR deferred (will retry after work): $(cat /tmp/sipag-pr-err.log)"
            fi
            tmux new-session -d -s claude \
                "claude --dangerously-skip-permissions -p \"\$PROMPT\"; \
                 echo \$? > /tmp/.claude-exit"
            touch /tmp/claude.log
            tmux pipe-pane -t claude -o "cat >> /tmp/claude.log"
            tail -f /tmp/claude.log &
            TAIL_PID=$!
            while tmux has-session -t claude 2>/dev/null; do sleep 1; done
            kill $TAIL_PID 2>/dev/null || true
            wait $TAIL_PID 2>/dev/null || true
            CLAUDE_EXIT=$(cat /tmp/.claude-exit 2>/dev/null || echo 1)
            if [[ "$CLAUDE_EXIT" -eq 0 ]]; then
                # Ensure PR exists after work is committed. Retry if the early creation failed.
                existing_pr=$(gh pr list --repo "'"${repo}"'" --head "$BRANCH" \
                    --state open --json number -q ".[0].number" 2>/dev/null || true)
                if [[ -z "$existing_pr" ]]; then
                    echo "[sipag] Retrying PR creation after work completion"
                    if gh pr create --repo "'"${repo}"'" \
                            --title "$ISSUE_TITLE" \
                            --body "$PR_BODY" \
                            --head "$BRANCH" 2>/tmp/sipag-pr-retry-err.log; then
                        echo "[sipag] PR created after work"
                    else
                        echo "[sipag] WARNING: PR creation failed after work: $(cat /tmp/sipag-pr-retry-err.log)"
                    fi
                fi
                gh pr ready "$BRANCH" --repo "'"${repo}"'" || true
                echo "[sipag] PR marked ready for review"
            fi
            exit "$CLAUDE_EXIT"
        ' > "$log_path" 2>&1

    local exit_code=$?
    local duration ended_at
    duration=$(( $(date +%s) - start_time ))
    ended_at=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

    if [[ $exit_code -eq 0 ]]; then
        # Success: remove in-progress (PR's "Closes #N" handles the rest)
        worker_transition_label "$repo" "$issue_num" "in-progress" ""
        echo "[#${issue_num}] DONE: $title"

        # Verify a PR was created; if the container's PR creation also failed, create one now.
        # This is the final safety net before the branch could be left orphaned.
        local pr_num pr_url
        pr_num=$(gh pr list --repo "$repo" --head "$branch" --state open --json number -q '.[0].number' 2>/dev/null || true)
        if [[ -z "$pr_num" ]]; then
            echo "[#${issue_num}] Post-run: no open PR found — creating recovery PR for branch ${branch}"
            if gh pr create --repo "$repo" \
                    --title "$title" \
                    --body "$pr_body" \
                    --head "$branch" 2>/dev/null; then
                echo "[#${issue_num}] Recovery PR created"
                pr_num=$(gh pr list --repo "$repo" --head "$branch" --state open --json number -q '.[0].number' 2>/dev/null || true)
            else
                echo "[#${issue_num}] WARNING: Recovery PR creation failed — branch ${branch} needs manual PR"
            fi
        fi
        pr_url=$(gh pr list --repo "$repo" --head "$branch" --json url -q '.[0].url' 2>/dev/null || true)

        # Update worker state: done
        _worker_update_state "$WORKER_REPO_SLUG" "$issue_num" "done" "$exit_code" \
            "$ended_at" "$duration" "${pr_num:-}" "${pr_url:-}"

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

        # Update worker state: failed
        _worker_update_state "$WORKER_REPO_SLUG" "$issue_num" "failed" "$exit_code" \
            "$ended_at" "$duration"

        # Hook: worker failed
        export SIPAG_EVENT="worker.failed"
        export SIPAG_EXIT_CODE="$exit_code"
        export SIPAG_LOG_PATH="$log_path"
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
        --name "sipag-pr-${pr_num}" \
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
            tmux new-session -d -s claude \
                "claude --dangerously-skip-permissions -p \"\$PROMPT\"; \
                 echo \$? > /tmp/.claude-exit"
            touch /tmp/claude.log
            tmux pipe-pane -t claude -o "cat >> /tmp/claude.log"
            tail -f /tmp/claude.log &
            TAIL_PID=$!
            while tmux has-session -t claude 2>/dev/null; do sleep 1; done
            kill $TAIL_PID 2>/dev/null || true
            wait $TAIL_PID 2>/dev/null || true
            exit "$(cat /tmp/.claude-exit 2>/dev/null || echo 1)"
        ' > "${WORKER_LOG_DIR}/${WORKER_REPO_SLUG}--pr-${pr_num}-iter.log" 2>&1

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

# Fix a PR with merge conflicts by merging main forward into the branch.
# If the merge is clean (no conflicts), pushes the merge commit automatically.
# If there are conflicts, runs Claude to resolve them.
#
# Always merges forward — never rebases, never force-pushes.
#
# $1: repo in OWNER/REPO format
# $2: pr_num — the conflicted PR to fix
worker_run_conflict_fix() {
    local repo="$1"
    local pr_num="$2"
    local title branch_name pr_body prompt

    worker_conflict_fix_mark_running "$pr_num"

    title=$(gh pr view "$pr_num" --repo "$repo" --json title -q '.title' 2>/dev/null)
    branch_name=$(gh pr view "$pr_num" --repo "$repo" --json headRefName -q '.headRefName' 2>/dev/null)

    echo "[PR #${pr_num}] Merging main forward: $title (branch: $branch_name)"

    pr_body=$(gh pr view "$pr_num" --repo "$repo" --json body -q '.body' 2>/dev/null || true)

    # Load prompt from template and substitute placeholders
    local _tpl_pr_num='{{PR_NUM}}' _tpl_pr_title='{{PR_TITLE}}'
    local _tpl_branch='{{BRANCH}}' _tpl_pr_body='{{PR_BODY}}'
    prompt=$(<"${_SIPAG_WORKER_LIB}/prompts/worker-conflict-fix.md")
    prompt="${prompt//${_tpl_pr_num}/${pr_num}}"
    prompt="${prompt//${_tpl_pr_title}/${title}}"
    prompt="${prompt//${_tpl_branch}/${branch_name}}"
    prompt="${prompt//${_tpl_pr_body}/${pr_body}}"

    PROMPT="$prompt" BRANCH="$branch_name" \
        ${WORKER_TIMEOUT_CMD:+$WORKER_TIMEOUT_CMD $WORKER_TIMEOUT} docker run --rm \
        --name "sipag-conflict-${pr_num}" \
        -e CLAUDE_CODE_OAUTH_TOKEN="${WORKER_OAUTH_TOKEN}" \
        -e ANTHROPIC_API_KEY="${WORKER_API_KEY}" \
        -e GH_TOKEN="$WORKER_GH_TOKEN" \
        -e PROMPT \
        -e BRANCH \
        "$WORKER_IMAGE" \
        bash -c '
            git clone "https://github.com/'"${repo}"'.git" /work && cd /work
            git config user.name "sipag"
            git config user.email "sipag@localhost"
            git remote set-url origin "https://x-access-token:${GH_TOKEN}@github.com/'"${repo}"'.git"
            git checkout "$BRANCH"
            git fetch origin main
            if git merge origin/main --no-edit; then
                # Clean merge — no conflicts, push the merge commit
                git push origin "$BRANCH"
                echo "[sipag] Merged main into $BRANCH (no conflicts)"
                exit 0
            fi
            # Conflicts detected — run Claude to resolve them
            echo "[sipag] Conflicts detected in $BRANCH, running Claude to resolve..."
            tmux new-session -d -s claude \
                "claude --dangerously-skip-permissions -p \"\$PROMPT\"; \
                 echo \$? > /tmp/.claude-exit"
            touch /tmp/claude.log
            tmux pipe-pane -t claude -o "cat >> /tmp/claude.log"
            tail -f /tmp/claude.log &
            TAIL_PID=$!
            while tmux has-session -t claude 2>/dev/null; do sleep 1; done
            kill $TAIL_PID 2>/dev/null || true
            wait $TAIL_PID 2>/dev/null || true
            exit "$(cat /tmp/.claude-exit 2>/dev/null || echo 1)"
        ' > "${WORKER_LOG_DIR}/${WORKER_REPO_SLUG}--pr-${pr_num}-conflict-fix.log" 2>&1

    local exit_code=$?
    worker_conflict_fix_mark_done "$pr_num"

    if [[ $exit_code -eq 0 ]]; then
        echo "[PR #${pr_num}] Conflict fix done: $title"
    else
        echo "[PR #${pr_num}] Conflict fix FAILED (exit ${exit_code}): $title"
    fi
}
