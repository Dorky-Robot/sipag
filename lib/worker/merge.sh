#!/usr/bin/env bash
# sipag — auto-merge clean sipag PRs
#
# Provides worker_auto_merge() which is called each polling cycle (after
# reconcile, before dispatch) to merge open sipag PRs that are CLEAN and
# ready. Prevents conflict cascades when multiple workers land PRs in rapid
# succession.
#
# Depends on globals set by config.sh: SIPAG_DIR, WORKER_AUTO_MERGE
# Depends on sipag_run_hook from github.sh

# shellcheck disable=SC2154  # Globals defined in config.sh / github.sh

# Auto-merge clean sipag PRs that pass validation.
#
# A PR is merged if ALL of the following are true:
#   - Branch starts with sipag/issue- (worker-created branch)
#   - mergeable == MERGEABLE and mergeStateStatus == CLEAN
#   - isDraft == false (worker finished pushing commits)
#   - reviewDecision is not CHANGES_REQUESTED
#
# Fires the on-pr-merged hook after each successful merge.
#
# $1: repo in OWNER/REPO format
worker_auto_merge() {
    local repo="$1"

    # Respect the auto_merge config setting (default: enabled)
    if [[ "${WORKER_AUTO_MERGE:-true}" != "true" ]]; then
        return 0
    fi

    # Find open PRs from sipag branches that are ready and clean.
    # mergeStateStatus == CLEAN means: all checks pass, no conflicts, no
    # blocking reviews. We check reviewDecision separately to be explicit
    # about CHANGES_REQUESTED exclusion.
    local -a candidates
    mapfile -t candidates < <(gh pr list --repo "$repo" --state open \
        --json number,headRefName,mergeable,mergeStateStatus,isDraft,reviewDecision \
        --jq '.[] | select(
            (.headRefName | startswith("sipag/issue-")) and
            .mergeable == "MERGEABLE" and
            .mergeStateStatus == "CLEAN" and
            .isDraft == false and
            .reviewDecision != "CHANGES_REQUESTED"
        ) | .number' 2>/dev/null)

    [[ ${#candidates[@]} -eq 0 ]] && return 0

    echo "[$(date +%H:%M:%S)] Auto-merge: ${#candidates[@]} candidate(s)"

    for pr_num in "${candidates[@]}"; do
        [[ -z "$pr_num" ]] && continue

        local title
        title=$(gh pr view "$pr_num" --repo "$repo" --json title -q '.title' 2>/dev/null)

        echo "[$(date +%H:%M:%S)] Auto-merging PR #${pr_num}: ${title}"

        if gh pr merge "$pr_num" --repo "$repo" --squash --subject "$title" 2>/dev/null; then
            echo "[$(date +%H:%M:%S)] Merged PR #${pr_num}"
            export SIPAG_EVENT="pr.auto-merged"
            export SIPAG_PR_NUM="$pr_num"
            export SIPAG_PR_TITLE="$title"
            sipag_run_hook "on-pr-merged"
        else
            echo "[$(date +%H:%M:%S)] Failed to merge PR #${pr_num} (may need manual review)"
        fi
    done
}
