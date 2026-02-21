#!/usr/bin/env bash
# sipag — worker configuration, initialization, and slug utility
#
# Defines all WORKER_* globals, loads ~/.sipag/config overrides, resolves
# credentials and timeout command, and provides the pure worker_slugify helper.
#
# Sourced by lib/worker.sh before all other worker submodules.

# shellcheck disable=SC2034  # Variables consumed by other worker submodules

SIPAG_DIR="${SIPAG_DIR:-$HOME/.sipag}"
WORKER_LOG_DIR="/tmp/sipag-backlog"

# Defaults (overridden by worker_load_config)
WORKER_BATCH_SIZE=4
WORKER_IMAGE="ghcr.io/dorky-robot/sipag-worker:latest"
WORKER_TIMEOUT=1800
WORKER_POLL_INTERVAL=120
WORKER_WORK_LABEL="${SIPAG_WORK_LABEL:-approved}"
WORKER_IN_PROGRESS_LABEL="in-progress"
WORKER_ONCE=0

# Per-repo overrides (populated by worker_fetch_repo_config)
WORKER_REPO_MODEL=""
WORKER_REPO_PROMPT_EXTRA=""

# Load global config from ~/.sipag/config (key=value format)
worker_load_config() {
    local config="${SIPAG_DIR}/config"
    [[ -f "$config" ]] || return 0

    while IFS='=' read -r key value; do
        key=$(echo "$key" | xargs)
        value=$(echo "$value" | xargs)
        [[ -z "$key" || "$key" == \#* ]] && continue
        case "$key" in
            batch_size)        WORKER_BATCH_SIZE="$value" ;;
            image)             WORKER_IMAGE="$value" ;;
            timeout)           WORKER_TIMEOUT="$value" ;;
            poll_interval)     WORKER_POLL_INTERVAL="$value" ;;
            work_label)        WORKER_WORK_LABEL="$value" ;;
            in_progress_label) WORKER_IN_PROGRESS_LABEL="$value" ;;
        esac
    done < "$config"
}

# Parse a value from a TOML file using python3's tomllib (Python 3.11+ stdlib).
# Handles scalars, integers, and multi-line strings transparently.
# Usage: sipag_toml_get <file> <section> <key>
# Returns the value, or empty string if not found or if tomllib is unavailable.
sipag_toml_get() {
    local file="$1" section="$2" key="$3"
    python3 - "$file" "$section" "$key" 2>/dev/null <<'PYEOF'
import sys
try:
    import tomllib
except ImportError:
    sys.exit(0)
with open(sys.argv[1], "rb") as f:
    data = tomllib.load(f)
value = data.get(sys.argv[2], {}).get(sys.argv[3])
if value is not None:
    print(value, end="")
PYEOF
}

# Fetch .sipag.toml from the repo root via the GitHub API and apply per-repo
# config overrides. Silently does nothing if the file is absent or python3's
# tomllib is unavailable.
#
# Resolution order (most specific wins):
#   1. .sipag.toml in repo root  (per-repo, handled here)
#   2. ~/.sipag/config           (global, handled by worker_load_config)
#   3. SIPAG_* env vars          (handled at shell startup)
#   4. Hardcoded defaults        (above)
#
# Overrides: WORKER_IMAGE, WORKER_TIMEOUT, WORKER_BATCH_SIZE,
#            WORKER_WORK_LABEL, WORKER_IN_PROGRESS_LABEL,
#            WORKER_REPO_MODEL, WORKER_REPO_PROMPT_EXTRA
worker_fetch_repo_config() {
    local repo="$1"
    local tmpfile
    tmpfile=$(mktemp /tmp/sipag-toml.XXXXXX)

    # Fetch .sipag.toml from GitHub; silently skip if absent or on any error
    if ! gh api "repos/${repo}/contents/.sipag.toml" \
            --jq '.content' 2>/dev/null \
            | base64 -d > "$tmpfile" 2>/dev/null \
        || [[ ! -s "$tmpfile" ]]; then
        rm -f "$tmpfile"
        return 0
    fi

    local val

    # [worker] section
    val=$(sipag_toml_get "$tmpfile" "worker" "image")
    [[ -n "$val" ]] && WORKER_IMAGE="$val"

    val=$(sipag_toml_get "$tmpfile" "worker" "timeout")
    [[ -n "$val" ]] && WORKER_TIMEOUT="$val"

    val=$(sipag_toml_get "$tmpfile" "worker" "batch_size")
    [[ -n "$val" ]] && WORKER_BATCH_SIZE="$val"

    val=$(sipag_toml_get "$tmpfile" "worker" "model")
    [[ -n "$val" ]] && WORKER_REPO_MODEL="$val"

    # [labels] section
    val=$(sipag_toml_get "$tmpfile" "labels" "work")
    [[ -n "$val" ]] && WORKER_WORK_LABEL="$val"

    val=$(sipag_toml_get "$tmpfile" "labels" "in_progress")
    [[ -n "$val" ]] && WORKER_IN_PROGRESS_LABEL="$val"

    # [prompts] section — multi-line strings handled transparently by tomllib
    val=$(sipag_toml_get "$tmpfile" "prompts" "extra")
    WORKER_REPO_PROMPT_EXTRA="$val"

    rm -f "$tmpfile"
    echo "[sipag] Loaded per-repo config from .sipag.toml"
    if [[ -n "$WORKER_IMAGE" ]];             then echo "[sipag]   image:        ${WORKER_IMAGE}"; fi
    if [[ -n "$WORKER_REPO_MODEL" ]];        then echo "[sipag]   model:        ${WORKER_REPO_MODEL}"; fi
    if [[ -n "$WORKER_REPO_PROMPT_EXTRA" ]]; then echo "[sipag]   extra prompt: (set)"; fi
}

# Initialize worker runtime state: log dir, seen file, timeout command, credentials
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
