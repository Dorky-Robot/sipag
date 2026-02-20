#!/usr/bin/env bash
# sipag start — prime session for agile workflow
#
# Launches an interactive Claude Code session to triage and refine
# GitHub issues into an approved backlog for sipag work.

# Resolve project root relative to this file
_START_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

start_run() {
    local repo="${1:?Usage: sipag start <owner/repo>}"
    local prompt_file="${_START_ROOT}/lib/prompts/start.md"

    if [[ ! -f "$prompt_file" ]]; then
        echo "Error: prompt file not found: $prompt_file" >&2
        return 1
    fi

    local prompt
    prompt="$(cat "$prompt_file")"

    run_claude "Start agile session for ${repo}" "${prompt}"$'\n\nRepo: '"${repo}"
}
