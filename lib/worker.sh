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

    # Load credentials
    if [[ ! -f "${SIPAG_DIR}/token" ]]; then
        echo "Error: no token found at ${SIPAG_DIR}/token"
        echo "Run: claude setup-token && cp ~/.claude/token ${SIPAG_DIR}/token"
        return 1
    fi
    WORKER_OAUTH_TOKEN=$(cat "${SIPAG_DIR}/token")
    WORKER_GH_TOKEN=$(gh auth token)
}

worker_is_seen() {
    grep -qx "$1" "$WORKER_SEEN_FILE" 2>/dev/null
}

worker_mark_seen() {
    echo "$1" >> "$WORKER_SEEN_FILE"
}

# Check if an issue already has a linked PR (open or merged)
# Verifies exact issue reference in PR body to avoid false positives
worker_has_pr() {
    local repo="$1" issue_num="$2"
    local candidates
    candidates=$(gh pr list --repo "$repo" --state all --search "closes #${issue_num}" --json number,body 2>/dev/null)
    echo "$candidates" | jq -e ".[] | select(.body // \"\" | test(\"(closes|fixes|resolves) #${issue_num}\\\\b\"))" &>/dev/null
}

# Close open issues whose work is already done (merged PR exists)
worker_reconcile() {
    local repo="$1"
    mapfile -t open_issues < <(gh issue list --repo "$repo" --state open --json number -q '.[].number' | sort -n)

    for issue in "${open_issues[@]}"; do
        # Check for merged PRs that reference this issue (exact word boundary match)
        local candidates pr_num=""
        candidates=$(gh pr list --repo "$repo" --state merged --search "closes #${issue}" \
            --json number,title,body -q '.[]')

        # Verify the PR body actually contains an exact reference to this issue number
        echo "$candidates" | jq -c '.' 2>/dev/null | while read -r pr; do
            [[ -z "$pr" ]] && continue
            local body
            body=$(echo "$pr" | jq -r '.body // ""')
            # Match exact issue ref: "closes #66" but not "closes #6"
            if echo "$body" | grep -qwE "(closes|fixes|resolves) #${issue}\\b"; then
                pr_num=$(echo "$pr" | jq -r '.number')
                local pr_title
                pr_title=$(echo "$pr" | jq -r '.title')
                echo "[$(date +%H:%M:%S)] Closing #${issue} — resolved by merged PR #${pr_num} (${pr_title})"
                gh issue close "$issue" --repo "$repo" --comment "Closed by merged PR #${pr_num}" 2>/dev/null
                worker_mark_seen "$issue"
                break
            fi
        done
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

# Run a single issue in a Docker container
worker_run_issue() {
    local repo="$1"
    local issue_num="$2"
    local title body prompt

    # Mark as in-progress so the spec is locked from edits
    worker_transition_label "$repo" "$issue_num" "$WORKER_WORK_LABEL" "in-progress"

    # Fetch the spec fresh right before starting (minimizes stale-spec window)
    title=$(gh issue view "$issue_num" --repo "$repo" --json title -q '.title')
    body=$(gh issue view "$issue_num" --repo "$repo" --json body -q '.body')

    echo "[#${issue_num}] Starting: $title"

    prompt="You are working on the repository at /work.

Your task:
${title}

${body}

Instructions:
- Create a new branch with a descriptive name
- Implement the changes
- Run any existing tests and make sure they pass
- Commit your changes with a clear commit message
- Push the branch and open a draft pull request early so progress is visible
- The PR title should match the task title
- The PR body should summarize what you changed and why
- When all work is complete, mark the pull request as ready for review
- The PR should close issue #${issue_num}"

    PROMPT="$prompt" ${WORKER_TIMEOUT_CMD:+$WORKER_TIMEOUT_CMD $WORKER_TIMEOUT} docker run --rm \
        -e CLAUDE_CODE_OAUTH_TOKEN="$WORKER_OAUTH_TOKEN" \
        -e GH_TOKEN="$WORKER_GH_TOKEN" \
        -e PROMPT \
        "$WORKER_IMAGE" \
        bash -c '
            git clone https://github.com/'"${repo}"'.git /work && cd /work
            git config user.name "sipag"
            git config user.email "sipag@localhost"
            claude --print --dangerously-skip-permissions -p "$PROMPT"
        ' > "${WORKER_LOG_DIR}/issue-${issue_num}.log" 2>&1

    local exit_code=$?
    if [[ $exit_code -eq 0 ]]; then
        # Success: remove in-progress (PR's "Closes #N" handles the rest)
        worker_transition_label "$repo" "$issue_num" "in-progress" ""
        echo "[#${issue_num}] DONE: $title"
    else
        # Failure: move back to approved for retry
        worker_transition_label "$repo" "$issue_num" "in-progress" "$WORKER_WORK_LABEL"
        echo "[#${issue_num}] FAILED (exit ${exit_code}): $title — returned to ${WORKER_WORK_LABEL}"
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

        # Fetch open issues, filter out already-seen
        local -a label_args=()
        [[ -n "$WORKER_WORK_LABEL" ]] && label_args=(--label "$WORKER_WORK_LABEL")
        mapfile -t all_issues < <(gh issue list --repo "$repo" --state open "${label_args[@]}" --json number -q '.[].number' | sort -n)

        local new_issues=()
        for issue in "${all_issues[@]}"; do
            if worker_is_seen "$issue"; then
                continue
            fi
            if worker_has_pr "$repo" "$issue"; then
                echo "[$(date +%H:%M:%S)] Skipping #${issue} (already has a PR)"
                worker_mark_seen "$issue"
                continue
            fi
            new_issues+=("$issue")
        done

        if [[ ${#new_issues[@]} -eq 0 ]]; then
            echo "[$(date +%H:%M:%S)] No new issues. ${#all_issues[@]} open (all picked up). Next poll in ${WORKER_POLL_INTERVAL}s..."
            sleep "$WORKER_POLL_INTERVAL"
            continue
        fi

        echo "[$(date +%H:%M:%S)] Found ${#new_issues[@]} new issues: ${new_issues[*]}"

        # Process in batches
        for ((i = 0; i < ${#new_issues[@]}; i += WORKER_BATCH_SIZE)); do
            local batch=("${new_issues[@]:i:WORKER_BATCH_SIZE}")
            echo "--- Batch: ${batch[*]} ---"

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

        echo "[$(date +%H:%M:%S)] Cycle done. Open PRs:"
        gh pr list --repo "$repo" --state open --json number,title \
            -q '.[] | "  #\(.number): \(.title)"'
        echo ""
        echo "[$(date +%H:%M:%S)] Next poll in ${WORKER_POLL_INTERVAL}s..."
        sleep "$WORKER_POLL_INTERVAL"
    done
}
