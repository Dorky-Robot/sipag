#!/usr/bin/env bash
# sipag — worker GitHub API operations
#
# Provides label management, PR status queries, PR iteration detection,
# reconciliation of merged PRs, and lifecycle hook execution.
#
# Depends on globals set by config.sh: SIPAG_DIR

# shellcheck disable=SC2154  # SIPAG_DIR set by config.sh

# Run a lifecycle hook script if it exists and is executable.
# Hooks live in ${SIPAG_DIR}/hooks/<name>. They run asynchronously so they
# never block the worker. Env vars must be exported by the caller before
# invoking this function.
sipag_run_hook() {
    local hook_name="$1"
    local hook_path="${SIPAG_DIR}/hooks/${hook_name}"
    [[ -x "$hook_path" ]] || return 0
    "$hook_path" &  # run async, don't block the worker
}

# Check if an issue already has a linked open or merged PR.
# Does NOT return true for PRs that were closed without merging, so that
# issues with abandoned PRs can be re-dispatched after re-approval.
worker_has_pr() {
    local repo="$1" issue_num="$2"
    local candidates
    candidates=$(gh pr list --repo "$repo" --state all --search "closes #${issue_num}" \
        --json number,body,state,mergedAt 2>/dev/null)
    echo "$candidates" | jq -e ".[] | select(
        (.body // \"\" | test(\"(closes|fixes|resolves) #${issue_num}\\\\b\")) and
        (.state == \"OPEN\" or .mergedAt != null)
    )" &>/dev/null
}

# Check if an issue has an open (not yet merged or closed) PR.
worker_has_open_pr() {
    local repo="$1" issue_num="$2"
    local candidates
    candidates=$(gh pr list --repo "$repo" --state open --search "closes #${issue_num}" \
        --json number,body 2>/dev/null)
    echo "$candidates" | jq -e ".[] | select(.body // \"\" | test(\"(closes|fixes|resolves) #${issue_num}\\\\b\"))" &>/dev/null
}

# Find open PRs that need another worker pass:
#   - a CHANGES_REQUESTED review submitted AFTER the most recent commit, OR
#   - any PR comment posted after the most recent commit
# Both conditions are anchored to the last commit date so that feedback already
# addressed by a worker (which pushed new commits) does not re-trigger iteration.
# This also covers the case where the PR author cannot formally request changes
# on their own PR, so feedback arrives as plain comments instead.
worker_find_prs_needing_iteration() {
    local repo="$1"
    gh pr list --repo "$repo" --state open \
        --json number,reviews,commits,comments \
        --jq '
            .[] |
            (
                if (.commits | length) > 0
                then .commits[-1].committedDate
                else "1970-01-01T00:00:00Z"
                end
            ) as $last_push |
            select(
                ((.reviews // []) | map(select(.state == "CHANGES_REQUESTED" and .submittedAt > $last_push)) | length > 0) or
                ((.comments // []) | map(select(.createdAt > $last_push)) | length > 0)
            ) |
            .number
        ' 2>/dev/null | sort -n
}

# Close in-progress issues whose worker-created PR has since been merged.
#
# Only examines issues labeled "in-progress" (set by worker_run_issue), not all
# open issues. Uses GitHub's timeline API to find an exact cross-reference from
# a merged PR — avoids the false positives produced by "gh pr list --search"
# fuzzy matching (e.g. searching for #66 returning PRs that mention #6).
worker_reconcile() {
    local repo="$1"
    mapfile -t inprogress < <(gh issue list --repo "$repo" --state open \
        --label "in-progress" --json number -q '.[].number' 2>/dev/null | sort -n)

    [[ ${#inprogress[@]} -eq 0 ]] && return 0

    for issue in "${inprogress[@]}"; do
        # Use the timeline API: look for a cross-referenced event from a merged PR.
        # This is an exact link — GitHub sets this when a PR body contains
        # "Closes #N" and that PR is merged. No fuzzy matching involved.
        local merged_pr
        merged_pr=$(gh api "repos/${repo}/issues/${issue}/timeline" \
            --jq '.[] | select(.event == "cross-referenced") |
                  select(.source.issue.pull_request.merged_at != null) |
                  .source.issue.number' 2>/dev/null | head -1)

        [[ -z "$merged_pr" ]] && continue

        local pr_title
        pr_title=$(gh pr view "$merged_pr" --repo "$repo" --json title -q '.title' 2>/dev/null)
        echo "[$(date +%H:%M:%S)] Closing #${issue} — resolved by merged PR #${merged_pr} (${pr_title})"
        gh issue close "$issue" --repo "$repo" --comment "Closed by merged PR #${merged_pr}" 2>/dev/null
        worker_mark_seen "$issue" "$repo"
    done
}

# Transition an issue's pipeline label: remove old, add new
# Usage: worker_transition_label <repo> <issue_num> <from_label> <to_label>
# Either label can be empty to skip that side of the swap.
worker_transition_label() {
    local repo="$1" issue_num="$2" from_label="$3" to_label="$4"
    [[ -n "$from_label" ]] && gh issue edit "$issue_num" --repo "$repo" --remove-label "$from_label" 2>/dev/null
    [[ -n "$to_label" ]]   && gh issue edit "$issue_num" --repo "$repo" --add-label "$to_label" 2>/dev/null
}
