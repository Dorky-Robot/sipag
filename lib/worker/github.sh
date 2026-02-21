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
#
# Also detects orphaned branches (sipag/issue-* with commits but no open PR)
# and creates recovery PRs for them, so no branch is left without a PR after
# a worker run that failed during PR creation.
worker_reconcile() {
    local repo="$1"
    mapfile -t inprogress < <(gh issue list --repo "$repo" --state open \
        --label "in-progress" --json number -q '.[].number' 2>/dev/null | sort -n)

    # Detect and recover orphaned branches every cycle, regardless of in-progress count
    worker_reconcile_orphaned_branches "$repo"

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

        local pr_title pr_url
        pr_title=$(gh pr view "$merged_pr" --repo "$repo" --json title -q '.title' 2>/dev/null)
        pr_url=$(gh pr view "$merged_pr" --repo "$repo" --json url -q '.url' 2>/dev/null || true)
        echo "[$(date +%H:%M:%S)] Closing #${issue} — resolved by merged PR #${merged_pr} (${pr_title})"
        gh issue close "$issue" --repo "$repo" --comment "Closed by merged PR #${merged_pr}" 2>/dev/null
        worker_mark_state_done "$repo" "$issue" "$merged_pr" "${pr_url:-}"

        # Delete the source branch for the merged PR to prevent stale branch accumulation
        local branch_name
        branch_name=$(gh pr view "$merged_pr" --repo "$repo" --json headRefName -q '.headRefName' 2>/dev/null || true)
        if [[ -n "$branch_name" ]]; then
            gh api -X DELETE "repos/${repo}/git/refs/heads/${branch_name}" 2>/dev/null || true
        fi
    done
}

# Scan for sipag/issue-* branches that have commits ahead of main but no open PR.
# Creates recovery PRs for any orphaned branches found. This runs every reconciliation
# cycle to catch workers that pushed commits but failed to create the PR.
worker_reconcile_orphaned_branches() {
    local repo="$1"

    # List all sipag/issue-* branches (capped at 100; covers any realistic backlog)
    local -a branches
    mapfile -t branches < <(gh api "repos/${repo}/branches?per_page=100" \
        --jq '.[].name | select(startswith("sipag/issue-"))' 2>/dev/null)

    for branch in "${branches[@]}"; do
        # Skip if an open PR already exists for this branch
        local open_pr
        open_pr=$(gh pr list --repo "$repo" --head "$branch" --state open \
            --json number -q '.[0].number' 2>/dev/null || true)
        [[ -n "$open_pr" ]] && continue

        # Skip and clean up if a merged PR already exists for this branch
        local merged_pr
        merged_pr=$(gh pr list --repo "$repo" --head "$branch" --state merged \
            --json number -q '.[0].number' 2>/dev/null || true)
        if [[ -n "$merged_pr" ]]; then
            echo "[$(date +%H:%M:%S)] Branch ${branch} already merged via PR #${merged_pr} — deleting stale branch"
            gh api -X DELETE "repos/${repo}/git/refs/heads/${branch}" 2>/dev/null || true
            continue
        fi

        # Skip if branch has no commits ahead of main
        local ahead_by
        ahead_by=$(gh api "repos/${repo}/compare/main...${branch}" \
            --jq '.ahead_by' 2>/dev/null || echo "0")
        [[ "${ahead_by:-0}" -le 0 ]] && continue

        # Extract issue number from branch pattern sipag/issue-NNN-slug
        local issue_num
        issue_num=$(echo "$branch" | sed -n 's|^sipag/issue-\([0-9]*\)-.*$|\1|p')
        [[ -z "$issue_num" ]] && continue

        # Fetch issue details to build the recovery PR
        local pr_title issue_body recovery_body
        pr_title=$(gh issue view "$issue_num" --repo "$repo" --json title \
            -q '.title' 2>/dev/null || echo "")
        [[ -z "$pr_title" ]] && pr_title="$branch"
        issue_body=$(gh issue view "$issue_num" --repo "$repo" --json body \
            -q '.body' 2>/dev/null || echo "")

        recovery_body="Closes #${issue_num}

${issue_body}

---
*This PR was created by sipag worker reconciliation (recovered orphaned branch).*"

        echo "[$(date +%H:%M:%S)] Orphaned branch detected: ${branch} (issue #${issue_num}: ${pr_title})"
        if gh pr create --repo "$repo" \
                --title "$pr_title" \
                --body "$recovery_body" \
                --head "$branch" 2>/dev/null; then
            echo "[$(date +%H:%M:%S)] Recovery PR created for branch ${branch}"
        else
            echo "[$(date +%H:%M:%S)] WARNING: Could not create recovery PR for ${branch}"
        fi
    done
}

# Check if an issue is currently open (not closed or merged).
# Returns 0 (true) for open issues, 1 (false) for closed issues.
# Used by worker_recover to skip label transitions on issues that have since been closed.
worker_issue_is_open() {
    local repo="$1" issue_num="$2"
    local state
    state=$(gh issue view "$issue_num" --repo "$repo" --json state -q '.state' 2>/dev/null || echo "")
    [[ "$state" == "OPEN" ]]
}

# Transition an issue's pipeline label: remove old, add new
# Usage: worker_transition_label <repo> <issue_num> <from_label> <to_label>
# Either label can be empty to skip that side of the swap.
# Uses || true so a non-zero exit from gh (e.g. closed issue) does not kill the worker under set -e.
worker_transition_label() {
    local repo="$1" issue_num="$2" from_label="$3" to_label="$4"
    # Curly braces + || true prevent set -e from killing the worker when the
    # issue is closed, missing, or the label doesn't exist.
    # The trailing `|| true` on each line ensures the function never returns
    # non-zero — without it, `[[ -n "" ]] && ...` returns 1 as the last
    # statement, which under set -e kills the caller.
    [[ -n "$from_label" ]] && { gh issue edit "$issue_num" --repo "$repo" --remove-label "$from_label" 2>/dev/null || true; } || true
    [[ -n "$to_label" ]]   && { gh issue edit "$issue_num" --repo "$repo" --add-label "$to_label" 2>/dev/null || true; } || true
}
