#!/usr/bin/env bash
# sipag — worker dedup using workers/*.json as single source of truth
#
# Uses ~/.sipag/workers/REPO_SLUG--ISSUE.json state files to determine whether
# an issue has been dispatched, is in flight, or has already completed.
# State values written by docker.sh: "enqueued", "running", "done", "failed".
#
# Also tracks PR iteration state using temp marker files (reset on restart).
#
# Depends on globals set by config.sh: SIPAG_DIR, WORKER_LOG_DIR

# shellcheck disable=SC2154  # SIPAG_DIR, WORKER_LOG_DIR set by config.sh

# Return the path to the worker state file for a given repo and issue.
worker_state_file() {
    local repo="$1" issue_num="$2"
    local repo_slug="${repo//\//--}"
    echo "${SIPAG_DIR}/workers/${repo_slug}--${issue_num}.json"
}

# Internal: check whether a state file has a specific status value.
_worker_state_has_status() {
    local state_file="$1" expected_status="$2"
    [[ -f "$state_file" ]] && jq -e --arg s "$expected_status" '.status == $s' "$state_file" &>/dev/null
}

# Check if issue has been completed successfully (state file status: done).
worker_is_completed() {
    local repo="$1" issue_num="$2"
    _worker_state_has_status "$(worker_state_file "$repo" "$issue_num")" "done"
}

# Check if issue is queued but waiting for a container to start (state file status: enqueued).
worker_is_enqueued() {
    local repo="$1" issue_num="$2"
    _worker_state_has_status "$(worker_state_file "$repo" "$issue_num")" "enqueued"
}

# Check if issue is currently in flight (state file status: enqueued, running, or recovering).
worker_is_in_flight() {
    local repo="$1" issue_num="$2"
    local sf
    sf=$(worker_state_file "$repo" "$issue_num")
    _worker_state_has_status "$sf" "enqueued" || _worker_state_has_status "$sf" "running" || _worker_state_has_status "$sf" "recovering"
}

# Check if issue's previous worker failed (state file status: failed).
worker_is_failed() {
    local repo="$1" issue_num="$2"
    _worker_state_has_status "$(worker_state_file "$repo" "$issue_num")" "failed"
}

# Create or update a worker state file marking the issue as done.
# Used by reconcile and the loop when an existing merged/open PR is discovered.
# $1: repo (OWNER/REPO), $2: issue_num, $3: pr_num (optional), $4: pr_url (optional)
worker_mark_state_done() {
    local repo="$1" issue_num="$2" pr_num="${3:-}" pr_url="${4:-}"
    local repo_slug="${repo//\//--}"
    local state_file="${SIPAG_DIR}/workers/${repo_slug}--${issue_num}.json"
    local now
    now=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

    mkdir -p "${SIPAG_DIR}/workers"

    if [[ -f "$state_file" ]]; then
        local tmp
        tmp=$(mktemp)
        jq \
            --arg status "done" \
            --arg ended_at "$now" \
            --arg pr_num "$pr_num" \
            --arg pr_url "$pr_url" \
            '.status = $status |
             .ended_at = (if .ended_at == null then $ended_at else .ended_at end) |
             .pr_num = (if $pr_num == "" then .pr_num else ($pr_num | tonumber) end) |
             .pr_url = (if $pr_url == "" then .pr_url else $pr_url end)' \
            "$state_file" > "$tmp" && mv "$tmp" "$state_file"
    else
        jq -n \
            --arg repo "${repo_slug/--//}" \
            --argjson issue_num "$issue_num" \
            --arg now "$now" \
            --arg pr_num "$pr_num" \
            --arg pr_url "$pr_url" \
            '{
                repo: $repo,
                issue_num: $issue_num,
                issue_title: "",
                branch: "",
                container_name: "",
                pr_num: (if $pr_num == "" then null else ($pr_num | tonumber) end),
                pr_url: (if $pr_url == "" then null else $pr_url end),
                status: "done",
                started_at: null,
                ended_at: $now,
                duration_s: null,
                exit_code: null,
                log_path: null
            }' > "$state_file"
    fi
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
