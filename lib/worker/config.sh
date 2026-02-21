#!/usr/bin/env bash
# sipag — worker configuration, initialization, and slug utility
#
# Defines all WORKER_* globals, loads ~/.sipag/config overrides, resolves
# credentials and timeout command, and provides the pure worker_slugify helper.
#
# Sourced by lib/worker.sh before all other worker submodules.

# shellcheck disable=SC2034  # Variables consumed by other worker submodules

SIPAG_DIR="${SIPAG_DIR:-$HOME/.sipag}"
WORKER_LOG_DIR="${SIPAG_DIR}/logs"

# Defaults (overridden by worker_load_config or env vars)
WORKER_BATCH_SIZE_MAX=5
WORKER_BATCH_SIZE="${SIPAG_BATCH_SIZE:-1}"
# Clamp env-var-supplied value to max
[[ "${WORKER_BATCH_SIZE}" -gt "${WORKER_BATCH_SIZE_MAX}" ]] && WORKER_BATCH_SIZE="${WORKER_BATCH_SIZE_MAX}"
WORKER_IMAGE="ghcr.io/dorky-robot/sipag-worker:latest"
WORKER_TIMEOUT=1800
WORKER_POLL_INTERVAL=120
WORKER_WORK_LABEL="${SIPAG_WORK_LABEL:-approved}"
WORKER_ONCE=0
WORKER_AUTO_MERGE=true
WORKER_DOC_REFRESH_INTERVAL="${SIPAG_DOC_REFRESH_INTERVAL:-10}"
WORKER_REPO_SLUG=""

# Load config
worker_load_config() {
    local config="${SIPAG_DIR}/config"
    [[ -f "$config" ]] || return 0

    while IFS='=' read -r key value; do
        key=$(echo "$key" | xargs)
        value=$(echo "$value" | xargs)
        [[ -z "$key" || "$key" == \#* ]] && continue
        case "$key" in
            batch_size)
                WORKER_BATCH_SIZE="$value"
                [[ "${WORKER_BATCH_SIZE}" -gt "${WORKER_BATCH_SIZE_MAX}" ]] && WORKER_BATCH_SIZE="${WORKER_BATCH_SIZE_MAX}"
                ;;
            image) WORKER_IMAGE="$value" ;;
            timeout) WORKER_TIMEOUT="$value" ;;
            poll_interval) WORKER_POLL_INTERVAL="$value" ;;
            work_label) WORKER_WORK_LABEL="$value" ;;
            auto_merge) WORKER_AUTO_MERGE="$value" ;;
            doc_refresh_interval) WORKER_DOC_REFRESH_INTERVAL="$value" ;;
        esac
    done < "$config"
}

# Initialize worker runtime state: log dir, timeout command, credentials
# $1: repo in OWNER/REPO format (e.g. "Dorky-Robot/sipag")
worker_init() {
    local repo="${1:-}"

    mkdir -p "${SIPAG_DIR}/workers"
    mkdir -p "${SIPAG_DIR}/logs"

    WORKER_LOG_DIR="${SIPAG_DIR}/logs"

    # Set per-repo slug (OWNER--REPO format to avoid path separator issues)
    if [[ -n "$repo" ]]; then
        WORKER_REPO_SLUG="${repo//\//--}"  # Dorky-Robot/sipag → Dorky-Robot--sipag
    fi

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
