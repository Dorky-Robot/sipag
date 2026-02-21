#!/usr/bin/env bash
# sipag — merge session context gatherer
#
# Gathers open PR context for a repository and outputs it to stdout,
# priming Claude for a conversational merge session.

SIPAG_DIR="${SIPAG_DIR:-$HOME/.sipag}"

# Display current worker status with focus on completed workers and their PRs
_merge_show_worker_status() {
    local workers_dir="${SIPAG_DIR}/workers"
    [[ -d "$workers_dir" ]] || return 0

    local running=0 done_count=0 failed=0
    local -a done_workers=()
    local f
    for f in "${workers_dir}"/*.json; do
        [[ -f "$f" ]] || continue
        local status pr_num issue_num issue_title
        status=$(jq -r '.status // ""' "$f")
        pr_num=$(jq -r 'if .pr_num != null then (.pr_num | tostring) else "" end' "$f")
        issue_num=$(jq -r '.issue_num // ""' "$f")
        issue_title=$(jq -r '.issue_title // ""' "$f")
        case "$status" in
            running) running=$(( running + 1 )) ;;
            done)
                done_count=$(( done_count + 1 ))
                if [[ -n "$pr_num" ]]; then
                    done_workers+=("  PR #${pr_num}: ${issue_title} (#${issue_num})")
                fi
                ;;
            failed) failed=$(( failed + 1 )) ;;
        esac
    done

    echo "## Worker Status"
    echo "${running} running · ${done_count} done · ${failed} failed"
    if [[ ${#done_workers[@]} -gt 0 ]]; then
        echo ""
        echo "Recently completed (PRs ready to review):"
        printf '%s\n' "${done_workers[@]}"
    fi
}

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
    _merge_show_worker_status

    echo ""
    cat "${SIPAG_ROOT}/lib/prompts/merge.md"
}
