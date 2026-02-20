#!/usr/bin/env bash
# sipag — merge workflow
#
# Conversational PR merge session: fetch open PRs, converse with Claude to
# review, approve, and merge pull requests.

SIPAG_DIR="${SIPAG_DIR:-$HOME/.sipag}"

merge_run() {
    local repo="${1:?Usage: sipag merge <owner/repo>}"
    local prompt_file="${SIPAG_ROOT}/lib/prompts/merge.md"

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

    echo "[merge] Launching PR merge session for ${repo}..."
    echo "[merge] Fetching open pull requests..."

    local prs
    prs=$(gh pr list --repo "$repo" --state open --json number,title,draft,reviewDecision \
        -q '.[] | "#\(.number): \(.title)\(if .draft then " [DRAFT]" else "" end)\(if .reviewDecision then " [\(.reviewDecision)]" else "" end)"' 2>/dev/null || true)

    local full_prompt
    full_prompt="$(printf '%s\n\nRepository: %s\n\nOpen pull requests:\n%s' "$prompt" "$repo" "${prs:-<none>}")"

    claude -p "$full_prompt"
}
