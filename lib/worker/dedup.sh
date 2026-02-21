#!/usr/bin/env bash
# sipag — worker dedup tracking (seen file) and PR in-flight state
#
# Tracks which issues have already been dispatched (seen file) and which
# PR iteration workers are currently running (temp marker files).
#
# Depends on globals set by config.sh: WORKER_SEEN_FILE, WORKER_LOG_DIR

# shellcheck disable=SC2154  # WORKER_SEEN_FILE, WORKER_LOG_DIR set by config.sh

# Track issues we've already picked up
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
