#!/usr/bin/env bash
# sipag — interactive Claude session launcher
#
# Gathers context from GitHub, builds a mode-specific system prompt,
# and launches an interactive Claude Code session (not --print mode).
#
# Usage (sourced as library):
#   source lib/start.sh
#   start_run <mode> <owner/repo>
#
# Usage (called directly):
#   bash lib/start.sh <mode> <owner/repo>
#
# Modes:
#   triage     — review open issues, set priorities/labels
#   refinement — break down approved issues into tasks
#   review     — review open pull requests
#
# Environment:
#   SIPAG_SKIP_PERMISSIONS — set to 0 to omit --dangerously-skip-permissions (default: 1)
#   SIPAG_MODEL            — claude model to use (e.g. claude-opus-4-5)

# ---------------------------------------------------------------------------
# Gather helpers — pull raw JSON from GitHub
# ---------------------------------------------------------------------------

# Fetch open issues for triage context
start_gather_triage() {
    local repo="$1"
    gh issue list \
        --repo "$repo" \
        --state open \
        --json number,title,labels,body,createdAt,assignees \
        --limit 50
}

# Fetch approved issues for refinement context
start_gather_refinement() {
    local repo="$1"
    gh issue list \
        --repo "$repo" \
        --state open \
        --label approved \
        --json number,title,body,labels,assignees,comments \
        --limit 20
}

# Fetch open pull requests for review context
start_gather_review() {
    local repo="$1"
    gh pr list \
        --repo "$repo" \
        --state open \
        --json number,title,body,author,createdAt,additions,deletions,files \
        --limit 20
}

# ---------------------------------------------------------------------------
# Prompt builders — assemble the system prompt from instructions + context
# ---------------------------------------------------------------------------

start_build_triage_prompt() {
    local repo="$1"
    local issues
    issues=$(start_gather_triage "$repo")

    cat <<EOF
You are an agile team assistant helping with issue triage for the ${repo} repository.

## Current Open Issues (GitHub)

${issues}

## Your Role

Help the team work through these issues by:
- Asking clarifying questions about priority, severity, and scope
- Suggesting appropriate labels (bug, enhancement, documentation, etc.)
- Identifying duplicates or closely related issues
- Breaking large issues into smaller, more actionable ones
- Clarifying vague requirements

You have access to the \`gh\` CLI tool and can update issues, add labels, add comments,
and close duplicates in real time as the conversation progresses.

Start by giving a brief summary of the current backlog state, then ask the team what
they want to focus on.
EOF
}

start_build_refinement_prompt() {
    local repo="$1"
    local issues
    issues=$(start_gather_refinement "$repo")

    cat <<EOF
You are an agile team assistant helping with backlog refinement for the ${repo} repository.

## Approved Issues Ready for Refinement (GitHub)

${issues}

## Your Role

Help the team refine these issues so they are ready for implementation:
- Break large issues into smaller, implementable tasks
- Write clear acceptance criteria
- Identify technical dependencies and blockers
- Estimate complexity (XS / S / M / L / XL)
- Suggest a logical implementation order

You have access to the \`gh\` CLI tool and can update issue bodies, add comments,
create sub-issues, and add labels in real time during the conversation.

Start by listing the approved issues and asking which one to refine first.
EOF
}

start_build_review_prompt() {
    local repo="$1"
    local prs
    prs=$(start_gather_review "$repo")

    cat <<EOF
You are a code review assistant for the ${repo} repository.

## Open Pull Requests (GitHub)

${prs}

## Your Role

Help the team review open pull requests:
- Summarise the changes and their purpose
- Identify potential issues, bugs, or missing test coverage
- Suggest improvements or ask the author clarifying questions
- Recommend approval or request-changes based on the discussion

You have access to the \`gh\` CLI tool and can add review comments, request changes,
or approve PRs in real time during the conversation.

Start by listing the open PRs and asking which one to review first.
EOF
}

# ---------------------------------------------------------------------------
# Main entry point
# ---------------------------------------------------------------------------

start_run() {
    local mode="$1"
    local repo="$2"

    if [[ -z "$mode" ]]; then
        echo "Usage: sipag start <mode> <owner/repo>" >&2
        echo "Available modes: triage, refinement, review" >&2
        return 1
    fi

    if [[ -z "$repo" ]]; then
        echo "Error: repo is required" >&2
        echo "Usage: sipag start <mode> <owner/repo>" >&2
        return 1
    fi

    local system_prompt
    case "$mode" in
        triage)
            system_prompt=$(start_build_triage_prompt "$repo")
            ;;
        refinement)
            system_prompt=$(start_build_refinement_prompt "$repo")
            ;;
        review)
            system_prompt=$(start_build_review_prompt "$repo")
            ;;
        *)
            echo "Unknown mode: $mode" >&2
            echo "Available modes: triage, refinement, review" >&2
            return 1
            ;;
    esac

    # Build claude argument list
    local -a args=()
    args+=(--system-prompt "$system_prompt")

    if [[ "${SIPAG_SKIP_PERMISSIONS:-1}" == "1" ]]; then
        args+=(--dangerously-skip-permissions)
    fi

    if [[ -n "${SIPAG_MODEL:-}" ]]; then
        args+=(--model "$SIPAG_MODEL")
    fi

    claude "${args[@]}"
}

# ---------------------------------------------------------------------------
# Direct invocation (not sourced)
# ---------------------------------------------------------------------------

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    start_run "$@"
fi
