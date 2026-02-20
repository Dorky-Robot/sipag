#!/usr/bin/env bash
# sipag merge — conversational PR merge session
#
# Launches an interactive Claude Code session to review, discuss,
# and merge open pull requests for a GitHub repository.

# Resolve project root relative to this file
_MERGE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

merge_run() {
    local repo="${1:?Usage: sipag merge <owner/repo>}"
    local prompt_file="${_MERGE_ROOT}/lib/prompts/merge.md"

    if [[ ! -f "$prompt_file" ]]; then
        echo "Error: prompt file not found: $prompt_file" >&2
        return 1
    fi

    local prompt
    prompt="$(cat "$prompt_file")"

    run_claude "Merge session for ${repo}" "${prompt}"$'\n\nRepo: '"${repo}"
}
