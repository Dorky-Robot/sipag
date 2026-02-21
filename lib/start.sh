#!/usr/bin/env bash
# sipag — start: gather GitHub context and prime a Claude Code session

set -euo pipefail

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
    gh label create "ready"        --repo "$repo" --color "0e8a16" --description "Pull when capacity is available — ready for a sipag worker" >/dev/null 2>&1 || true
    gh label create "in-progress"  --repo "$repo" --color "fbca04" --description "Worker is actively building this — do not edit"      >/dev/null 2>&1 || true
    gh label create "needs-review" --repo "$repo" --color "d93f0b" --description "Worker finished — PR awaits human review"            >/dev/null 2>&1 || true
    gh label create "P0"          --repo "$repo" --color "b60205" --description "Critical — blocks everything"                      >/dev/null 2>&1 || true
    gh label create "P1"          --repo "$repo" --color "e4e669" --description "Important — needed for the cycle"                  >/dev/null 2>&1 || true
    gh label create "P2"          --repo "$repo" --color "0075ca" --description "Normal — nice to have soon"                       >/dev/null 2>&1 || true
    gh label create "P3"          --repo "$repo" --color "bfd4f2" --description "Low — when we get to it"                          >/dev/null 2>&1 || true
}

# Ensure the sipag workflow section exists in the target repo's CLAUDE.md.
# Uses the GitHub Contents API — idempotent. Creates, appends, or updates
# the section delimited by <!-- sipag:workflow:start/end --> markers.
_start_ensure_workflow_doc() {
    local repo="$1"
    local template_path="${SIPAG_ROOT}/lib/prompts/workflow-reference.md"

    if [[ ! -f "$template_path" ]]; then
        echo "Warning: workflow template not found at ${template_path}" >&2
        return 0
    fi

    local desired
    desired=$(cat "$template_path")

    # Fetch current CLAUDE.md (base64-encoded content + sha)
    local api_resp
    api_resp=$(gh api "repos/${repo}/contents/CLAUDE.md" 2>/dev/null) || api_resp=""

    if [[ -z "$api_resp" ]]; then
        # CLAUDE.md doesn't exist — create it with the workflow section
        local encoded
        encoded=$(printf '%s\n' "$desired" | base64)
        gh api --method PUT "repos/${repo}/contents/CLAUDE.md" \
            -f message="chore: add sipag workflow reference" \
            -f content="$encoded" >/dev/null 2>&1 || true
        return 0
    fi

    # Decode existing content
    local sha current
    sha=$(printf '%s' "$api_resp" | jq -r '.sha')
    current=$(printf '%s' "$api_resp" | jq -r '.content' | base64 -d 2>/dev/null) || current=""

    # Check for existing markers
    if printf '%s' "$current" | grep -q '<!-- sipag:workflow:start -->'; then
        # Extract existing workflow section
        local existing
        existing=$(printf '%s' "$current" | sed -n '/<!-- sipag:workflow:start -->/,/<!-- sipag:workflow:end -->/p')
        if [[ "$existing" == "$desired" ]]; then
            # Already up to date
            return 0
        fi
        # Replace between markers
        local updated
        updated=$(printf '%s' "$current" | awk -v new="$desired" '
            /<!-- sipag:workflow:start -->/ { print new; skip=1; next }
            /<!-- sipag:workflow:end -->/ { skip=0; next }
            !skip { print }
        ')
    else
        # No markers — append workflow section
        local updated
        updated=$(printf '%s\n\n%s' "$current" "$desired")
    fi

    local encoded
    encoded=$(printf '%s\n' "$updated" | base64)
    gh api --method PUT "repos/${repo}/contents/CLAUDE.md" \
        -f message="chore: update sipag workflow reference" \
        -f content="$encoded" \
        -f sha="$sha" >/dev/null 2>&1 || true
}

# Print GitHub context for one repo (without the workflow prompt)
_start_print_repo_context() {
    local repo="$1"

    _start_ensure_labels "$repo"
    _start_ensure_workflow_doc "$repo"

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
    echo "| \`ready\`        | Pull when capacity is available — ready for a sipag worker to pick up |"
    echo "| \`in-progress\`  | Worker is actively building this — do not edit the issue while set |"
    echo "| \`needs-review\` | Worker finished — PR awaits human review |"
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

# Entry point called by the Rust CLI. Routes to start_run (single repo)
# or start_run_all (no args).
start_run_wrapper() {
    if [[ $# -gt 0 ]]; then
        start_run "$1"
    else
        start_run_all
    fi
}
