#!/usr/bin/env bash
# sipag — refresh-docs: auto-maintain ARCHITECTURE.md and VISION.md
#
# Generates or updates ARCHITECTURE.md from the live codebase via a Claude
# worker running in Docker. VISION.md is never overwritten — only flagged for
# staleness or drafted if missing.
#
# Public functions:
#   refresh_docs_is_stale <repo>       Returns 0 if stale/missing, 1 if current
#   refresh_docs_run <repo> [--check]  Refresh docs; --check skips if up-to-date
#
# WORKER_* globals must be set before calling refresh_docs_run. Call
# worker_load_config + worker_init first, or use cmd_refresh_docs in bin/sipag
# which does this automatically.

# Resolve lib dir so we can find prompt templates regardless of how we were sourced.
_SIPAG_WORKER_LIB="${_SIPAG_WORKER_LIB:-$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)}"
SIPAG_DIR="${SIPAG_DIR:-$HOME/.sipag}"

# Check whether ARCHITECTURE.md is stale relative to the last merged PR.
# A file is considered stale when it has never been committed (missing), or when
# a PR was merged into the default branch more recently than the last commit that
# touched ARCHITECTURE.md.
#
# Returns 0 (stale or missing), 1 (up-to-date).
# $1: repo in "owner/repo" format
refresh_docs_is_stale() {
    local repo="$1"

    # Date of the most recent commit that touched ARCHITECTURE.md (empty if none).
    # gh api with --jq outputs the literal string "null" when no commits are found,
    # so we filter both empty string and "null" as "not found".
    local arch_commit_date
    arch_commit_date=$(gh api "repos/${repo}/commits?path=ARCHITECTURE.md&per_page=1" \
        --jq '.[0].commit.committer.date' 2>/dev/null || true)

    if [[ -z "$arch_commit_date" || "$arch_commit_date" == "null" ]]; then
        return 0  # File has never been committed — definitely stale
    fi

    # Date of the most recently merged PR (empty or "null" if none).
    local last_merge_date
    last_merge_date=$(gh pr list --repo "$repo" --state merged --limit 1 \
        --json mergedAt -q '.[0].mergedAt' 2>/dev/null || true)

    if [[ -z "$last_merge_date" || "$last_merge_date" == "null" ]]; then
        return 1  # No merged PRs — docs are current enough
    fi

    # ISO 8601 UTC strings compare correctly as strings (lexicographic == chronological)
    if [[ "$last_merge_date" > "$arch_commit_date" ]]; then
        return 0  # A PR was merged after the last doc update — stale
    fi

    return 1  # Docs are newer than (or same age as) the last merge — up-to-date
}

# Run a doc refresh: spin up a Docker worker that analyzes the codebase and
# generates/updates ARCHITECTURE.md and VISION.md, then opens a PR.
#
# When --check is passed, the refresh is skipped if ARCHITECTURE.md is already
# up-to-date. Without --check the refresh always runs.
#
# $1: repo in "owner/repo" format
# $2 (optional): --check
refresh_docs_run() {
    local repo="$1"
    local check_mode="${2:-}"

    if [[ "$check_mode" == "--check" ]]; then
        if refresh_docs_is_stale "$repo"; then
            echo "[refresh-docs] ARCHITECTURE.md is stale or missing — refreshing..."
        else
            echo "[refresh-docs] ARCHITECTURE.md is up-to-date."
            return 0
        fi
    fi

    # Resolve credentials from WORKER_* globals (set by worker_init) or fallbacks
    local gh_token oauth_token api_key
    gh_token="${WORKER_GH_TOKEN:-$(gh auth token 2>/dev/null || true)}"
    oauth_token="${WORKER_OAUTH_TOKEN:-}"
    api_key="${WORKER_API_KEY:-}"

    # Load Claude token from ~/.sipag/token if WORKER_OAUTH_TOKEN is not set
    if [[ -z "$oauth_token" && -s "${SIPAG_DIR}/token" ]]; then
        oauth_token=$(cat "${SIPAG_DIR}/token")
    fi
    if [[ -z "$oauth_token" && -z "$api_key" && -n "${ANTHROPIC_API_KEY:-}" ]]; then
        api_key="${ANTHROPIC_API_KEY}"
    fi

    local log_dir="${WORKER_LOG_DIR:-${SIPAG_DIR}/logs}"
    mkdir -p "$log_dir"
    local log_path="${log_dir}/refresh-docs-${repo//\//--}.log"

    # Load prompt template and substitute {{REPO}}
    local prompt
    local _tpl_repo='{{REPO}}'
    prompt=$(<"${_SIPAG_WORKER_LIB}/prompts/refresh-docs.md")
    prompt="${prompt//${_tpl_repo}/${repo}}"

    # Datestamped branch: multiple runs on the same day reuse the same branch
    local branch
    branch="sipag/refresh-docs-$(date +%Y%m%d)"
    local image="${WORKER_IMAGE:-ghcr.io/dorky-robot/sipag-worker:latest}"
    local timeout_cmd="${WORKER_TIMEOUT_CMD:-}"
    local timeout="${WORKER_TIMEOUT:-1800}"
    local container_name="sipag-refresh-docs-${repo//\//--}"

    echo "[refresh-docs] Starting doc refresh for ${repo} (branch: ${branch})..."

    PROMPT="$prompt" BRANCH="$branch" \
        ${timeout_cmd:+$timeout_cmd $timeout} docker run --rm \
        --name "$container_name" \
        -e CLAUDE_CODE_OAUTH_TOKEN="$oauth_token" \
        -e ANTHROPIC_API_KEY="$api_key" \
        -e GH_TOKEN="$gh_token" \
        -e PROMPT \
        -e BRANCH \
        "$image" \
        bash -c '
            git clone "https://github.com/'"${repo}"'.git" /work && cd /work
            git config user.name "sipag"
            git config user.email "sipag@localhost"
            git remote set-url origin "https://x-access-token:${GH_TOKEN}@github.com/'"${repo}"'.git"
            # Reuse existing branch if it was already pushed today, otherwise create it
            git checkout -b "$BRANCH" 2>/dev/null || git checkout "$BRANCH"
            git push -u origin "$BRANCH" 2>/dev/null || true
            tmux new-session -d -s claude \
                "claude --dangerously-skip-permissions -p \"\$PROMPT\"; \
                 echo \$? > /tmp/.claude-exit"
            touch /tmp/claude.log
            tmux pipe-pane -t claude -o "cat >> /tmp/claude.log"
            tail -f /tmp/claude.log &
            TAIL_PID=$!
            while tmux has-session -t claude 2>/dev/null; do sleep 1; done
            kill $TAIL_PID 2>/dev/null || true
            wait $TAIL_PID 2>/dev/null || true
            exit "$(cat /tmp/.claude-exit 2>/dev/null || echo 1)"
        ' >> "$log_path" 2>&1

    local exit_code=$?
    if [[ $exit_code -eq 0 ]]; then
        echo "[refresh-docs] Done. Log: ${log_path}"
    else
        echo "[refresh-docs] Failed (exit ${exit_code}). Log: ${log_path}"
    fi

    return "$exit_code"
}
