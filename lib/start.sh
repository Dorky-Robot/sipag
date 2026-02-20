#!/usr/bin/env bash
# sipag — start workflow
#
# Interactive agile session: fetch open issues, converse with Claude to triage,
# refine, and approve issues for the worker to pick up.

SIPAG_DIR="${SIPAG_DIR:-$HOME/.sipag}"

start_run() {
    local repo="${1:?Usage: sipag start <owner/repo>}"
    local prompt_file="${SIPAG_ROOT}/lib/prompts/start.md"

    # Check prerequisites
    if ! command -v gh >/dev/null 2>&1; then
        echo "Error: gh CLI required. Install from https://cli.github.com"
        return 1
    fi

    if ! command -v claude >/dev/null 2>&1; then
        echo "Error: claude CLI required. Install from https://claude.ai/code"
        return 1
    fi

    if ! gh auth status >/dev/null 2>&1; then
        echo "Error: gh not authenticated. Run: gh auth login"
        return 1
    fi

    if [[ ! -f "$prompt_file" ]]; then
        echo "Error: prompt template not found at ${prompt_file}"
        return 1
    fi

    local prompt
    prompt=$(cat "$prompt_file")

    echo "[start] Launching agile triage session for ${repo}..."
    echo "[start] Fetching open issues..."

    local issues
    issues=$(gh issue list --repo "$repo" --state open --json number,title,labels \
        -q '.[] | "#\(.number): \(.title) [\(.labels | map(.name) | join(", "))]"' 2>/dev/null || true)

    local full_prompt
    full_prompt="$(printf '%s\n\nRepository: %s\n\nOpen issues:\n%s' "$prompt" "$repo" "${issues:-<none>}")"

    claude -p "$full_prompt"
}
