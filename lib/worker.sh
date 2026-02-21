#!/usr/bin/env bash
# sipag — Docker worker for GitHub issues
#
# Polls a GitHub repo for open issues, spins up isolated Docker containers
# to work on them via Claude Code, creates PRs. Runs continuously until killed.

SIPAG_DIR="${SIPAG_DIR:-$HOME/.sipag}"
WORKER_LOG_DIR="/tmp/sipag-backlog"

# Defaults (overridden by config)
WORKER_BATCH_SIZE=4
WORKER_IMAGE="sipag-worker:latest"
WORKER_TIMEOUT=1800
WORKER_POLL_INTERVAL=120
WORKER_WORK_LABEL="${SIPAG_WORK_LABEL:-approved}"

# Load config
worker_load_config() {
    local config="${SIPAG_DIR}/config"
    [[ -f "$config" ]] || return 0

    while IFS='=' read -r key value; do
        key=$(echo "$key" | xargs)
        value=$(echo "$value" | xargs)
        [[ -z "$key" || "$key" == \#* ]] && continue
        case "$key" in
            batch_size) WORKER_BATCH_SIZE="$value" ;;
            image) WORKER_IMAGE="$value" ;;
            timeout) WORKER_TIMEOUT="$value" ;;
            poll_interval) WORKER_POLL_INTERVAL="$value" ;;
            work_label) WORKER_WORK_LABEL="$value" ;;
        esac
    done < "$config"
}

# Track issues we've already picked up
worker_init() {
    mkdir -p "$WORKER_LOG_DIR"
    WORKER_SEEN_FILE="${SIPAG_DIR}/seen"
    touch "$WORKER_SEEN_FILE"

    # Resolve timeout command (gtimeout on macOS, timeout on Linux)
    WORKER_TIMEOUT_CMD="timeout"
    command -v gtimeout &>/dev/null && WORKER_TIMEOUT_CMD="gtimeout"
    command -v "$WORKER_TIMEOUT_CMD" &>/dev/null || WORKER_TIMEOUT_CMD=""

    # Load credentials: token file takes priority, ANTHROPIC_API_KEY is fallback
    WORKER_OAUTH_TOKEN=""
    WORKER_API_KEY=""
    if [[ -s "${SIPAG_DIR}/token" ]]; then
        WORKER_OAUTH_TOKEN=$(cat "${SIPAG_DIR}/token")
    elif [[ -n "${ANTHROPIC_API_KEY:-}" ]]; then
        WORKER_API_KEY="${ANTHROPIC_API_KEY}"
    fi
    WORKER_GH_TOKEN=$(gh auth token)
}

worker_is_seen() {
    grep -qx "$1" "$WORKER_SEEN_FILE" 2>/dev/null
}

worker_mark_seen() {
    echo "$1" >> "$WORKER_SEEN_FILE"
}

# Remove an issue from the seen file so it can be re-dispatched
worker_unsee() {
    local issue="$1"
    [[ -f "$WORKER_SEEN_FILE" ]] || return 0
    grep -vx "$issue" "$WORKER_SEEN_FILE" > "${WORKER_SEEN_FILE}.tmp" \
        && mv "${WORKER_SEEN_FILE}.tmp" "$WORKER_SEEN_FILE" \
        || rm -f "${WORKER_SEEN_FILE}.tmp"
}

# Track PR iteration state using temp files (reset on process restart)
worker_pr_is_running() {
    [[ -f "${WORKER_LOG_DIR}/pr-${1}-running" ]]
}

worker_pr_mark_running() {
    touch "${WORKER_LOG_DIR}/pr-${1}-running"
}

worker_pr_mark_done() {
    rm -f "${WORKER_LOG_DIR}/pr-${1}-running"
}

# Check if an issue already has a linked open or merged PR.
# Does NOT return true for PRs that were closed without merging, so that
# issues with abandoned PRs can be re-dispatched after re-approval.
worker_has_pr() {
    local repo="$1" issue_num="$2"
    local candidates
    candidates=$(gh pr list --repo "$repo" --state all --search "closes #${issue_num}" \
        --json number,body,state,mergedAt 2>/dev/null)
    echo "$candidates" | jq -e ".[] | select(
        (.body // \"\" | test(\"(closes|fixes|resolves) #${issue_num}\\\\b\")) and
        (.state == \"OPEN\" or .mergedAt != null)
    )" &>/dev/null
}

# Check if an issue has an open (not yet merged or closed) PR.
worker_has_open_pr() {
    local repo="$1" issue_num="$2"
    local candidates
    candidates=$(gh pr list --repo "$repo" --state open --search "closes #${issue_num}" \
        --json number,body 2>/dev/null)
    echo "$candidates" | jq -e ".[] | select(.body // \"\" | test(\"(closes|fixes|resolves) #${issue_num}\\\\b\"))" &>/dev/null
}

# Find open PRs that need another worker pass:
#   - formal CHANGES_REQUESTED review, OR
#   - any PR comment posted after the most recent commit
# This covers the common case where the PR author cannot request changes on
# their own PR, so feedback arrives as plain comments rather than reviews.
worker_find_prs_needing_iteration() {
    local repo="$1"
    gh pr list --repo "$repo" --state open \
        --json number,reviewDecision,commits,comments \
        --jq '
            .[] |
            (
                if (.commits | length) > 0
                then .commits[-1].committedDate
                else "1970-01-01T00:00:00Z"
                end
            ) as $last_push |
            select(
                (.reviewDecision == "CHANGES_REQUESTED") or
                ((.comments // []) | map(select(.createdAt > $last_push)) | length > 0)
            ) |
            .number
        ' 2>/dev/null | sort -n
}

# Close in-progress issues whose worker-created PR has since been merged.
#
# Only examines issues labeled "in-progress" (set by worker_run_issue), not all
# open issues. Uses GitHub's timeline API to find an exact cross-reference from
# a merged PR — avoids the false positives produced by "gh pr list --search"
# fuzzy matching (e.g. searching for #66 returning PRs that mention #6).
worker_reconcile() {
    local repo="$1"
    mapfile -t inprogress < <(gh issue list --repo "$repo" --state open \
        --label "in-progress" --json number -q '.[].number' 2>/dev/null | sort -n)

    [[ ${#inprogress[@]} -eq 0 ]] && return 0

    for issue in "${inprogress[@]}"; do
        # Use the timeline API: look for a cross-referenced event from a merged PR.
        # This is an exact link — GitHub sets this when a PR body contains
        # "Closes #N" and that PR is merged. No fuzzy matching involved.
        local merged_pr
        merged_pr=$(gh api "repos/${repo}/issues/${issue}/timeline" \
            --jq '.[] | select(.event == "cross-referenced") |
                  select(.source.issue.pull_request.merged_at != null) |
                  .source.issue.number' 2>/dev/null | head -1)

        [[ -z "$merged_pr" ]] && continue

        local pr_title
        pr_title=$(gh pr view "$merged_pr" --repo "$repo" --json title -q '.title' 2>/dev/null)
        echo "[$(date +%H:%M:%S)] Closing #${issue} — resolved by merged PR #${merged_pr} (${pr_title})"
        gh issue close "$issue" --repo "$repo" --comment "Closed by merged PR #${merged_pr}" 2>/dev/null
        worker_mark_seen "$issue"
    done
}

# Transition an issue's pipeline label: remove old, add new
# Usage: worker_transition_label <repo> <issue_num> <from_label> <to_label>
# Either label can be empty to skip that side of the swap.
worker_transition_label() {
    local repo="$1" issue_num="$2" from_label="$3" to_label="$4"
    [[ -n "$from_label" ]] && gh issue edit "$issue_num" --repo "$repo" --remove-label "$from_label" 2>/dev/null
    [[ -n "$to_label" ]]   && gh issue edit "$issue_num" --repo "$repo" --add-label "$to_label" 2>/dev/null
}

# Convert an issue title into a URL-safe branch name slug (max 50 chars)
worker_slugify() {
    local title="$1"
    echo "$title" \
        | tr '[:upper:]' '[:lower:]' \
        | sed 's/[^a-z0-9]/-/g' \
        | tr -s '-' \
        | sed 's/^-//' \
        | sed 's/-$//' \
        | cut -c1-50
}

# Run a single issue in a Docker container
worker_run_issue() {
    local repo="$1"
    local issue_num="$2"
    local title body branch slug pr_body prompt

    # Mark as in-progress so the spec is locked from edits
    worker_transition_label "$repo" "$issue_num" "$WORKER_WORK_LABEL" "in-progress"

    # Fetch the spec fresh right before starting (minimizes stale-spec window)
    title=$(gh issue view "$issue_num" --repo "$repo" --json title -q '.title')
    body=$(gh issue view "$issue_num" --repo "$repo" --json body -q '.body')

    echo "[#${issue_num}] Starting: $title"

    # Generate branch name and draft PR body before entering container
    slug=$(worker_slugify "$title")
    branch="sipag/issue-${issue_num}-${slug}"

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
    if [[ $exit_code -eq 0 ]]; then
        # Success: remove in-progress (PR's "Closes #N" handles the rest)
        worker_transition_label "$repo" "$issue_num" "in-progress" ""
        echo "[#${issue_num}] DONE: $title"
    else
        # Failure: move back to approved for retry (draft PR stays open showing progress)
        worker_transition_label "$repo" "$issue_num" "in-progress" "$WORKER_WORK_LABEL"
        echo "[#${issue_num}] FAILED (exit ${exit_code}): $title — returned to ${WORKER_WORK_LABEL}"
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
- Read the review feedback carefully
- Make targeted changes that address the feedback
- Do NOT rewrite the PR from scratch — make surgical fixes
- Commit with a message that references the feedback
- Push to the same branch (git push origin ${branch_name})"

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
    if [[ $exit_code -eq 0 ]]; then
        echo "[PR #${pr_num}] DONE iterating: $title"
    else
        echo "[PR #${pr_num}] FAILED iteration (exit ${exit_code}): $title"
    fi
}

# Main polling loop
worker_loop() {
    local repo="$1"

    echo "sipag work"
    echo "Repo: ${repo}"
    echo "Label: ${WORKER_WORK_LABEL:-<all>}"
    echo "Batch size: ${WORKER_BATCH_SIZE}"
    echo "Poll interval: ${WORKER_POLL_INTERVAL}s"
    echo "Logs: ${WORKER_LOG_DIR}/"
    echo "Started: $(date)"
    echo ""

    while true; do
        # Reconcile: close issues that already have merged PRs
        worker_reconcile "$repo"

        # Fetch open issues with the work label
        local -a label_args=()
        [[ -n "$WORKER_WORK_LABEL" ]] && label_args=(--label "$WORKER_WORK_LABEL")
        mapfile -t all_issues < <(gh issue list --repo "$repo" --state open "${label_args[@]}" --json number -q '.[].number' | sort -n)

        local new_issues=()
        for issue in "${all_issues[@]}"; do
            if worker_is_seen "$issue"; then
                # Seen issues: skip only while an open PR is still in progress.
                # If re-labeled approved after a closed/failed PR, re-queue.
                if worker_has_open_pr "$repo" "$issue"; then
                    continue
                fi
                echo "[$(date +%H:%M:%S)] Re-queuing #${issue} (re-approved, no open PR)"
                worker_unsee "$issue"
            elif worker_has_pr "$repo" "$issue"; then
                # Never seen but already has an open or merged PR (e.g. from another session)
                echo "[$(date +%H:%M:%S)] Skipping #${issue} (already has a PR)"
                worker_mark_seen "$issue"
                continue
            fi
            new_issues+=("$issue")
        done

        # Find open PRs with review feedback requesting changes
        local prs_to_iterate=()
        mapfile -t prs_needing_changes < <(worker_find_prs_needing_iteration "$repo")
        for pr_num in "${prs_needing_changes[@]}"; do
            if worker_pr_is_running "$pr_num"; then
                echo "[$(date +%H:%M:%S)] Skipping PR #${pr_num} iteration (already in progress)"
                continue
            fi
            prs_to_iterate+=("$pr_num")
        done

        if [[ ${#new_issues[@]} -eq 0 && ${#prs_to_iterate[@]} -eq 0 ]]; then
            local total_open open_prs
            total_open=$(gh issue list --repo "$repo" --state open --limit 500 --json number --jq 'length' 2>/dev/null || echo "?")
            open_prs=$(gh pr list --repo "$repo" --state open --json number --jq 'length' 2>/dev/null || echo "?")
            echo "[$(date +%H:%M:%S)] ${#all_issues[@]} approved, ${total_open} open total, ${open_prs} PRs open. Next poll in ${WORKER_POLL_INTERVAL}s..."
            sleep "$WORKER_POLL_INTERVAL"
            continue
        fi

        # Process new issues in batches
        if [[ ${#new_issues[@]} -gt 0 ]]; then
            echo "[$(date +%H:%M:%S)] Found ${#new_issues[@]} new issues: ${new_issues[*]}"

            for ((i = 0; i < ${#new_issues[@]}; i += WORKER_BATCH_SIZE)); do
                local batch=("${new_issues[@]:i:WORKER_BATCH_SIZE}")
                echo "--- Issue batch: ${batch[*]} ---"

                for issue in "${batch[@]}"; do
                    worker_mark_seen "$issue"
                done

                local pids=()
                for issue in "${batch[@]}"; do
                    worker_run_issue "$repo" "$issue" &
                    pids+=($!)
                done

                for pid in "${pids[@]}"; do
                    wait "$pid" 2>/dev/null || true
                done

                echo "--- Batch complete ---"
                echo ""
            done
        fi

        # Process PR iterations in batches
        if [[ ${#prs_to_iterate[@]} -gt 0 ]]; then
            echo "[$(date +%H:%M:%S)] Found ${#prs_to_iterate[@]} PRs needing iteration: ${prs_to_iterate[*]}"

            for ((i = 0; i < ${#prs_to_iterate[@]}; i += WORKER_BATCH_SIZE)); do
                local iter_batch=("${prs_to_iterate[@]:i:WORKER_BATCH_SIZE}")
                echo "--- PR iteration batch: ${iter_batch[*]} ---"

                local pids=()
                for pr_num in "${iter_batch[@]}"; do
                    worker_run_pr_iteration "$repo" "$pr_num" &
                    pids+=($!)
                done

                for pid in "${pids[@]}"; do
                    wait "$pid" 2>/dev/null || true
                done

                echo "--- PR iteration batch complete ---"
                echo ""
            done
        fi

        echo "[$(date +%H:%M:%S)] Cycle done. Open PRs:"
        gh pr list --repo "$repo" --state open --json number,title \
            -q '.[] | "  #\(.number): \(.title)"'
        echo ""
        echo "[$(date +%H:%M:%S)] Next poll in ${WORKER_POLL_INTERVAL}s..."
        sleep "$WORKER_POLL_INTERVAL"
    done
}
