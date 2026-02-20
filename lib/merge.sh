#!/usr/bin/env bash
# sipag — merge session context gatherer
#
# Gathers open PR context for a repository and outputs it to stdout,
# priming Claude for a conversational merge session.

merge_run() {
    local repo="$1"

    echo "=== sipag: loading merge context for ${repo} ==="
    echo ""

    echo "## Open Pull Requests"
    gh pr list --repo "$repo" --state open \
        --json number,title,body,reviewDecision,additions,deletions,headRefName,mergeable,statusCheckRollup --limit 30

    echo ""
    echo "## Recent Commits on Main"
    gh api "repos/${repo}/commits?per_page=10" \
        --jq '.[] | {sha: .sha[0:7], message: (.commit.message | split("\n")[0]), date: .commit.author.date}' 2>/dev/null

    echo ""
    cat "${SIPAG_ROOT}/lib/prompts/merge.md"
}
