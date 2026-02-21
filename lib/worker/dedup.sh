#!/usr/bin/env bash
# sipag — worker dedup tracking (per-repo seen files) and PR in-flight state
#
# Tracks which issues have already been dispatched (per-repo seen files under
# ~/.sipag/seen/OWNER--REPO) and which PR iteration workers are currently
# running (marker files in ~/.sipag/logs/).
#
# Depends on globals set by config.sh: WORKER_SEEN_DIR, WORKER_LOG_DIR

# shellcheck disable=SC2154  # WORKER_SEEN_DIR, WORKER_LOG_DIR set by config.sh

# Return the path to the seen file for a given repo (OWNER--REPO format)
worker_seen_file() {
    local repo="$1"
    local repo_slug
    repo_slug=$(echo "$repo" | sed 's|/|--|g')
    echo "${WORKER_SEEN_DIR}/${repo_slug}"
}

# Track issues we've already picked up (per-repo)
worker_is_seen() {
    local issue="$1" repo="$2"
    local seen_file
    seen_file=$(worker_seen_file "$repo")
    grep -qx "$issue" "$seen_file" 2>/dev/null
}

worker_mark_seen() {
    local issue="$1" repo="$2"
    local seen_file
    seen_file=$(worker_seen_file "$repo")
    echo "$issue" >> "$seen_file"
}

# Remove an issue from the seen file so it can be re-dispatched
worker_unsee() {
    local issue="$1" repo="$2"
    local seen_file
    seen_file=$(worker_seen_file "$repo")
    [[ -f "$seen_file" ]] || return 0
    grep -vx "$issue" "$seen_file" > "${seen_file}.tmp" \
        && mv "${seen_file}.tmp" "$seen_file" \
        || rm -f "${seen_file}.tmp"
}

# Track PR iteration state using marker files in the log dir
worker_pr_is_running() {
    local pr_num="$1" repo="$2"
    local repo_slug
    repo_slug=$(echo "$repo" | sed 's|/|--|g')
    [[ -f "${WORKER_LOG_DIR}/${repo_slug}--pr-${pr_num}-running" ]]
}

worker_pr_mark_running() {
    local pr_num="$1" repo="$2"
    local repo_slug
    repo_slug=$(echo "$repo" | sed 's|/|--|g')
    touch "${WORKER_LOG_DIR}/${repo_slug}--pr-${pr_num}-running"
}

worker_pr_mark_done() {
    local pr_num="$1" repo="$2"
    local repo_slug
    repo_slug=$(echo "$repo" | sed 's|/|--|g')
    rm -f "${WORKER_LOG_DIR}/${repo_slug}--pr-${pr_num}-running"
}
