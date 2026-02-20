#!/usr/bin/env bash
# sipag — start: gather GitHub context and prime a Claude Code session

start_run() {
    local repo="${1:?Usage: sipag start <owner/repo>}"

    echo "=== sipag: loading context for ${repo} ==="
    echo ""

    echo "## Open Issues"
    gh issue list --repo "${repo}" --state open \
        --json number,title,body,labels,comments,createdAt --limit 100

    echo ""
    echo "## Open Pull Requests"
    gh pr list --repo "${repo}" --state open \
        --json number,title,body,reviewDecision,additions,deletions --limit 20

    echo ""
    echo "## Labels"
    gh label list --repo "${repo}" --json name,description --limit 100

    echo ""
    echo "## Recently Closed Issues"
    gh issue list --repo "${repo}" --state closed \
        --json number,title,labels,closedAt --limit 20

    echo ""
    echo "## Repository Structure"
    gh api "repos/${repo}/git/trees/HEAD?recursive=1" \
        --jq '[.tree[].path]' 2>/dev/null | head -200

    echo ""
    cat "${SIPAG_ROOT}/lib/prompts/start.md"
}
