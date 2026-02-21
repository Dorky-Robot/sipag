#!/usr/bin/env bash
# sipag — start: gather GitHub context and prime a Claude Code session

SIPAG_DIR="${SIPAG_DIR:-$HOME/.sipag}"

# Display current worker status from ~/.sipag/workers/*.json
_start_show_worker_status() {
    local workers_dir="${SIPAG_DIR}/workers"
    [[ -d "$workers_dir" ]] || return 0

    local printed_header=0
    local f
    for f in "${workers_dir}"/*.json; do
        [[ -f "$f" ]] || continue
        if [[ $printed_header -eq 0 ]]; then
            echo "## Worker Status"
            printed_header=1
        fi
        jq -r '"  [\(.status)] #\(.issue_num): \(.issue_title) [\(.repo)]"' "$f" 2>/dev/null
    done
}

# Ensure the standard sipag labels exist on a repo (idempotent — silently skips existing labels)
_start_ensure_labels() {
    local repo="$1"
    gh label create "approved"    --repo "$repo" --color "0e8a16" --description "Ready for sipag work — greenlit for development"   >/dev/null 2>&1 || true
    gh label create "in-progress" --repo "$repo" --color "fbca04" --description "Worker is actively building this — do not edit"    >/dev/null 2>&1 || true
    gh label create "P0"          --repo "$repo" --color "b60205" --description "Critical — blocks everything"                      >/dev/null 2>&1 || true
    gh label create "P1"          --repo "$repo" --color "e4e669" --description "Important — needed for the cycle"                  >/dev/null 2>&1 || true
    gh label create "P2"          --repo "$repo" --color "0075ca" --description "Normal — nice to have soon"                       >/dev/null 2>&1 || true
    gh label create "P3"          --repo "$repo" --color "bfd4f2" --description "Low — when we get to it"                          >/dev/null 2>&1 || true
}

# Print GitHub context for one repo (without the workflow prompt)
_start_print_repo_context() {
    local repo="$1"

    _start_ensure_labels "$repo"

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
    echo "## Sipag Label Legend"
    echo "| Label | Purpose |"
    echo "|-------|---------|"
    echo "| \`approved\`    | Gates worker pickup — label an issue here to queue it for a Docker worker |"
    echo "| \`in-progress\` | Worker is actively building this — do not edit the issue while set |"
    echo "| \`P0\`          | Critical — blocks everything; should be rare |"
    echo "| \`P1\`          | Important — needed for the cycle |"
    echo "| \`P2\`          | Normal — nice to have soon |"
    echo "| \`P3\`          | Low — when we get to it |"
    echo ""
    echo "## Labels on this repo"
    gh label list --repo "${repo}" --json name,description --limit 100

    echo ""
    echo "## Recently Closed Issues"
    gh issue list --repo "${repo}" --state closed \
        --json number,title,labels,closedAt --limit 20

    echo ""
    echo "## Repository Structure"
    gh api "repos/${repo}/git/trees/HEAD?recursive=1" \
        --jq '[.tree[].path]' 2>/dev/null | head -200
}

# Detect owner/repo from the current git directory's origin remote.
# Returns empty string if not in a git repo or no GitHub origin.
_start_detect_repo() {
    local url
    url=$(git remote get-url origin 2>/dev/null) || return 0
    url="${url%.git}"
    url="${url#https://github.com/}"
    url="${url#git@github.com:}"
    echo "$url"
}

# Gather context from all repos in repos.conf (or detect from git) and combine into one session
start_run_all() {
    local -a repos=()

    # Try repos.conf first
    local conf="${SIPAG_DIR}/repos.conf"
    if [[ -f "$conf" ]]; then
        local name url
        while IFS='=' read -r name url; do
            name="${name// /}"
            url="${url// /}"
            [[ -z "$name" || "$name" == \#* ]] && continue
            url="${url%.git}"
            url="${url#https://github.com/}"
            repos+=("$url")
        done < "$conf"
    fi

    # Fall back to detecting from current git repo
    if [[ ${#repos[@]} -eq 0 ]]; then
        local detected
        detected=$(_start_detect_repo)
        if [[ -n "$detected" ]]; then
            repos+=("$detected")
        else
            echo "Error: Not in a git repo and no repos registered."
            echo "  Run from a git repo, or: sipag repo add <name> <url>"
            return 1
        fi
    fi

    echo "=== sipag: loading context for ${#repos[@]} repos: ${repos[*]} ==="
    echo ""

    local repo
    for repo in "${repos[@]}"; do
        _start_print_repo_context "$repo"
        echo ""
        echo "---"
        echo ""
    done

    _start_show_worker_status
    echo ""
    cat "${SIPAG_ROOT}/lib/prompts/start.md"
}

start_run() {
    local repo="${1:?Usage: sipag start <owner/repo>}"
    _start_print_repo_context "$repo"
    echo ""
    cat "${SIPAG_ROOT}/lib/prompts/start.md"
}
