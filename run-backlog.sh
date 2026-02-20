#!/usr/bin/env bash
# run-backlog.sh — continuously process open sipag issues in parallel batches
#
# One approval, then walk away. Wake up to PRs.
# Polls for new issues every POLL_INTERVAL seconds.
# Does NOT merge anything — just creates PRs for review.
# Tracks which issues have already been picked up to avoid duplicates.

set -euo pipefail

SIPAG_DIR="${SIPAG_DIR:-$HOME/.sipag}"
OAUTH_TOKEN=$(cat "$SIPAG_DIR/token")
GH_TOKEN_VAL=$(gh auth token)
REPO="${1:-Dorky-Robot/sipag}"
LOG_DIR="/tmp/sipag-backlog"

# Load config
BATCH_SIZE=4
IMAGE="sipag-worker:latest"
TIMEOUT=1800
POLL_INTERVAL=120  # seconds between polls

if [[ -f "$SIPAG_DIR/config" ]]; then
    while IFS='=' read -r key value; do
        key=$(echo "$key" | xargs)
        value=$(echo "$value" | xargs)
        [[ -z "$key" || "$key" == \#* ]] && continue
        case "$key" in
            batch_size) BATCH_SIZE="$value" ;;
            image) IMAGE="$value" ;;
            timeout) TIMEOUT="$value" ;;
            poll_interval) POLL_INTERVAL="$value" ;;
        esac
    done < "$SIPAG_DIR/config"
fi

mkdir -p "$LOG_DIR"

# Track issues we've already started so we don't double-process
SEEN_FILE="${LOG_DIR}/.seen"
touch "$SEEN_FILE"

is_seen() {
    grep -qx "$1" "$SEEN_FILE" 2>/dev/null
}

mark_seen() {
    echo "$1" >> "$SEEN_FILE"
}

# Use gtimeout on macOS, timeout on Linux
TIMEOUT_CMD="timeout"
command -v gtimeout &>/dev/null && TIMEOUT_CMD="gtimeout"
command -v "$TIMEOUT_CMD" &>/dev/null || TIMEOUT_CMD=""

run_issue() {
    local issue_num="$1"
    local title body prompt

    title=$(gh issue view "$issue_num" --repo "$REPO" --json title -q '.title')
    body=$(gh issue view "$issue_num" --repo "$REPO" --json body -q '.body')

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

    PROMPT="$prompt" ${TIMEOUT_CMD:+$TIMEOUT_CMD $TIMEOUT} docker run --rm \
        -e CLAUDE_CODE_OAUTH_TOKEN="$OAUTH_TOKEN" \
        -e GH_TOKEN="$GH_TOKEN_VAL" \
        -e PROMPT \
        "$IMAGE" \
        bash -c '
            git clone https://github.com/'"${REPO}"'.git /work && cd /work
            git config user.name "sipag"
            git config user.email "sipag@localhost"
            claude --print --dangerously-skip-permissions -p "$PROMPT"
        ' > "${LOG_DIR}/issue-${issue_num}.log" 2>&1

    local exit_code=$?
    if [[ $exit_code -eq 0 ]]; then
        echo "[#${issue_num}] DONE: $title"
    else
        echo "[#${issue_num}] FAILED (exit ${exit_code}): $title"
    fi
}

echo "sipag backlog runner (continuous)"
echo "Repo: ${REPO}"
echo "Batch size: ${BATCH_SIZE}"
echo "Poll interval: ${POLL_INTERVAL}s"
echo "Logs: ${LOG_DIR}/"
echo "Started: $(date)"
echo ""

while true; do
    # Fetch open issues, filter out ones we've already started
    mapfile -t ALL_ISSUES < <(gh issue list --repo "$REPO" --state open --json number -q '.[].number' | sort -n)

    NEW_ISSUES=()
    for issue in "${ALL_ISSUES[@]}"; do
        is_seen "$issue" || NEW_ISSUES+=("$issue")
    done

    if [[ ${#NEW_ISSUES[@]} -eq 0 ]]; then
        echo "[$(date +%H:%M:%S)] No new issues. ${#ALL_ISSUES[@]} open (all already picked up). Polling again in ${POLL_INTERVAL}s..."
        sleep "$POLL_INTERVAL"
        continue
    fi

    echo "[$(date +%H:%M:%S)] Found ${#NEW_ISSUES[@]} new issues: ${NEW_ISSUES[*]}"

    # Process new issues in batches
    for ((i = 0; i < ${#NEW_ISSUES[@]}; i += BATCH_SIZE)); do
        batch=("${NEW_ISSUES[@]:i:BATCH_SIZE}")
        echo "--- Batch: ${batch[*]} ---"

        # Mark all in this batch as seen before starting
        for issue in "${batch[@]}"; do
            mark_seen "$issue"
        done

        pids=()
        for issue in "${batch[@]}"; do
            run_issue "$issue" &
            pids+=($!)
        done

        # Wait for batch to complete before starting next
        for pid in "${pids[@]}"; do
            wait "$pid" 2>/dev/null || true
        done

        echo "--- Batch complete ---"
        echo ""
    done

    echo "[$(date +%H:%M:%S)] Batch cycle done. PRs open:"
    gh pr list --repo "$REPO" --state open --json number,title \
        -q '.[] | "  #\(.number): \(.title)"'
    echo ""
    echo "[$(date +%H:%M:%S)] Polling again in ${POLL_INTERVAL}s..."
    sleep "$POLL_INTERVAL"
done
