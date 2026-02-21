#!/usr/bin/env bash
# sipag-state.sh — update the shared worker state file from inside a container.
#
# The state file is mounted from the host at $STATE_FILE.
# Uses jq for atomic field-level updates so the host Rust process and container
# don't clobber each other's writes.
#
# Usage:
#   sipag-state heartbeat              Update last_heartbeat to now
#   sipag-state phase "cloning repo"   Set the current phase
#   sipag-state pr <num> <url>         Record PR number and URL
#   sipag-state status <status>        Set status (running, done, failed)
#   sipag-state finish <exit_code>     Set status=done/failed, ended_at, exit_code, duration
set -euo pipefail

STATE_FILE="${STATE_FILE:-}"

if [[ -z "$STATE_FILE" ]]; then
    # No state file mounted — silently no-op so scripts work without it.
    exit 0
fi

if ! command -v jq &>/dev/null; then
    exit 0
fi

now_utc() {
    date -u +"%Y-%m-%dT%H:%M:%SZ"
}

# Atomic update: read → modify → write to tmp → mv (rename is atomic on POSIX).
update_state() {
    local filter="$1"
    if [[ ! -f "$STATE_FILE" ]]; then
        return 0
    fi
    local tmp="${STATE_FILE}.tmp.$$"
    jq "$filter" "$STATE_FILE" > "$tmp" && mv "$tmp" "$STATE_FILE"
}

case "${1:-}" in
    heartbeat)
        update_state ".last_heartbeat = \"$(now_utc)\""
        ;;
    phase)
        shift
        local_phase="${1:-}"
        update_state ".phase = \"${local_phase}\" | .last_heartbeat = \"$(now_utc)\""
        ;;
    pr)
        shift
        pr_num="${1:-}"
        pr_url="${2:-}"
        update_state ".pr_num = ${pr_num} | .pr_url = \"${pr_url}\" | .last_heartbeat = \"$(now_utc)\""
        ;;
    status)
        shift
        new_status="${1:-}"
        update_state ".status = \"${new_status}\" | .last_heartbeat = \"$(now_utc)\""
        ;;
    finish)
        shift
        exit_code="${1:-0}"
        if [[ "$exit_code" -eq 0 ]]; then
            final_status="done"
        else
            final_status="failed"
        fi
        # Compute duration from started_at if available.
        started_at=$(jq -r '.started_at // empty' "$STATE_FILE" 2>/dev/null || true)
        duration_s="null"
        if [[ -n "$started_at" ]]; then
            start_epoch=$(date -d "$started_at" +%s 2>/dev/null || date -j -f "%Y-%m-%dT%H:%M:%SZ" "$started_at" +%s 2>/dev/null || echo "")
            if [[ -n "$start_epoch" ]]; then
                now_epoch=$(date +%s)
                duration_s=$(( now_epoch - start_epoch ))
            fi
        fi
        update_state ".status = \"${final_status}\" | .exit_code = ${exit_code} | .ended_at = \"$(now_utc)\" | .duration_s = ${duration_s} | .last_heartbeat = \"$(now_utc)\""
        ;;
    *)
        echo "Usage: sipag-state {heartbeat|phase|pr|status|finish}" >&2
        exit 1
        ;;
esac
