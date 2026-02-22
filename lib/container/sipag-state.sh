#!/usr/bin/env bash
# sipag-state.sh — update the shared worker state file from inside a container.
#
# The state file is mounted from the host at $STATE_FILE.
# Uses jq --arg / --argjson for safe, injection-free field updates so the
# host Rust process and container don't clobber each other's writes.
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
# Signature: update_state filter [jq-options...]
# Pass --arg / --argjson options after the filter; they are forwarded to jq.
update_state() {
    local filter="$1"
    shift
    if [[ ! -f "$STATE_FILE" ]]; then
        return 0
    fi
    local tmp="${STATE_FILE}.tmp.$$"
    # Clean up temp file on function return (success or error).
    # SC2064: intentional — expand $tmp now so the trap path is baked in.
    # shellcheck disable=SC2064
    trap "rm -f '${tmp}'" RETURN
    jq "$@" "$filter" "$STATE_FILE" > "$tmp" && mv "$tmp" "$STATE_FILE"
}

case "${1:-}" in
    heartbeat)
        update_state '.last_heartbeat = $ts' --arg ts "$(now_utc)"
        ;;
    phase)
        shift
        update_state '.phase = $p | .last_heartbeat = $ts' \
            --arg p "${1:-}" \
            --arg ts "$(now_utc)"
        ;;
    pr)
        shift
        pr_num="${1:-}"
        pr_url="${2:-}"
        # Validate pr_num is an integer before inserting into JSON.
        if [[ ! "$pr_num" =~ ^[0-9]+$ ]]; then
            echo "sipag-state pr: expected numeric PR number, got: '${pr_num}'" >&2
            exit 1
        fi
        update_state '.pr_num = ($n | tonumber) | .pr_url = $u | .last_heartbeat = $ts' \
            --arg n "$pr_num" \
            --arg u "$pr_url" \
            --arg ts "$(now_utc)"
        ;;
    status)
        shift
        update_state '.status = $s | .last_heartbeat = $ts' \
            --arg s "${1:-}" \
            --arg ts "$(now_utc)"
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
        # Use --argjson for numeric/null types so they serialize correctly.
        update_state '.status = $s | .exit_code = $c | .ended_at = $ts | .duration_s = $d | .last_heartbeat = $ts' \
            --arg s "$final_status" \
            --argjson c "$exit_code" \
            --arg ts "$(now_utc)" \
            --argjson d "$duration_s"
        ;;
    *)
        echo "Usage: sipag-state {heartbeat|phase|pr|status|finish}" >&2
        exit 1
        ;;
esac
