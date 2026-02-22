#!/usr/bin/env bash
set -euo pipefail

# ── Resolve prompt/PR-body from file (avoids OS arg size limits) ─────────────
# PR_BODY is small enough to keep as an env var (used by gh pr create below).
if [[ -n "${PR_BODY_FILE:-}" ]] && [[ -f "$PR_BODY_FILE" ]]; then
    PR_BODY="$(cat "$PR_BODY_FILE")"
    export PR_BODY
fi

# ── Resolve issue list ───────────────────────────────────────────────────────
# ISSUE_NUMS (space-separated) is set for grouped workers; fall back to ISSUE_NUM.
ALL_ISSUES="${ISSUE_NUMS:-${ISSUE_NUM:-}}"

# ── Label management helpers ─────────────────────────────────────────────────
# The container owns label transitions so they happen atomically with the work.
# WORK_LABEL is passed via env var from the host.

# Transition a label on a single issue — idempotent and lifecycle-aware.
#
# Checks current labels before modifying:
#   - Only removes a label if it is currently present (no-op otherwise).
#   - Only adds a label if it is not already present.
#   - Lifecycle ordering (approved < in-progress < needs-review): adding a label
#     that would regress the issue (e.g. in-progress when needs-review is present)
#     is silently skipped, even after accounting for the removal.
transition_label_one() {
    local issue="${1:-}" remove="${2:-}" add="${3:-}"
    if [[ -z "$issue" ]]; then return 0; fi

    # Fetch current labels for idempotency checks (one API call).
    local current_labels
    current_labels=$(gh issue view "$issue" --repo "${REPO}" --json labels \
        -q '.labels[].name' 2>/dev/null || true)

    if [[ -n "$remove" ]]; then
        # Only remove if the label is actually present.
        if echo "$current_labels" | grep -qx "$remove" 2>/dev/null; then
            gh issue edit "$issue" --repo "${REPO}" --remove-label "$remove" 2>/dev/null || true
        fi
    fi

    if [[ -n "$add" ]]; then
        # Skip if already present (idempotent).
        if echo "$current_labels" | grep -qx "$add" 2>/dev/null; then
            return 0
        fi

        # Compute effective labels after the remove, then check lifecycle regression.
        # Lifecycle: approved (0) < in-progress (1) < needs-review (2).
        local effective_labels="$current_labels"
        if [[ -n "$remove" ]]; then
            effective_labels=$(echo "$current_labels" | grep -vx "$remove" 2>/dev/null || true)
        fi

        local should_add=true
        if [[ "$add" == "in-progress" ]]; then
            # Don't regress from needs-review → in-progress.
            if echo "$effective_labels" | grep -qx "needs-review" 2>/dev/null; then
                echo "[sipag] Skipping: cannot add 'in-progress' to #$issue (already at 'needs-review')" >&2
                should_add=false
            fi
        elif [[ "$add" == "approved" || "$add" == "${WORK_LABEL:-ready}" ]]; then
            # Don't regress from in-progress or needs-review → work label.
            if echo "$effective_labels" | grep -qx "needs-review" 2>/dev/null || \
               echo "$effective_labels" | grep -qx "in-progress" 2>/dev/null; then
                echo "[sipag] Skipping: cannot add '$add' to #$issue (already at later lifecycle stage)" >&2
                should_add=false
            fi
        fi

        if [[ "$should_add" == "true" ]]; then
            gh issue edit "$issue" --repo "${REPO}" --add-label "$add" 2>/dev/null || true
        fi
    fi
}

# Transition a label on all issues in ALL_ISSUES.
transition_label() {
    local remove="${1:-}" add="${2:-}"
    for issue in $ALL_ISSUES; do
        transition_label_one "$issue" "$remove" "$add"
    done
}

# ── Start: transition ready → in-progress ─────────────────────────────────────
transition_label "${WORK_LABEL:-ready}" "in-progress"

sipag-state phase "cloning repo" || true

git clone "https://github.com/${REPO}.git" /work && cd /work
git config user.name "sipag"
git config user.email "sipag@localhost"
git remote set-url origin "https://x-access-token:${GH_TOKEN}@github.com/${REPO}.git"

sipag-state phase "creating branch" || true

git checkout -b "$BRANCH"

# Clean up stale remote branch from a previous cycle (e.g. closed PR).
# Without this, git push fails with "non-fast-forward" when the branch name
# is deterministic and a previous cycle used the same name.
if git ls-remote --exit-code --heads origin "$BRANCH" >/dev/null 2>&1; then
    echo "[sipag] Deleting stale remote branch: $BRANCH"
    git push origin --delete "$BRANCH" 2>/dev/null || true
fi

git push -u origin "$BRANCH"

sipag-state phase "opening draft PR" || true

if gh pr create --repo "${REPO}" \
        --title "$ISSUE_TITLE" \
        --body "$PR_BODY" \
        --draft \
        --head "$BRANCH" 2>/tmp/sipag-pr-err.log; then
    echo "[sipag] Draft PR opened: branch=$BRANCH"
    # Record PR info in state file.
    pr_json=$(gh pr list --repo "${REPO}" --head "$BRANCH" --state open --json number,url -q '.[0]' 2>/dev/null || true)
    if [[ -n "$pr_json" ]]; then
        pr_num=$(echo "$pr_json" | jq -r '.number // empty' 2>/dev/null || true)
        pr_url=$(echo "$pr_json" | jq -r '.url // empty' 2>/dev/null || true)
        if [[ -n "$pr_num" ]]; then
            sipag-state pr "$pr_num" "$pr_url" || true
        fi
    fi
else
    echo "[sipag] Draft PR deferred (will retry after work): $(cat /tmp/sipag-pr-err.log)"
fi

sipag-state phase "running claude" || true

# Heartbeat in background: update state file every 30s while Claude is running.
(
    while true; do
        sleep 30
        sipag-state heartbeat || true
    done
) &
HEARTBEAT_PID=$!

# Pass prompt via pipe (not CLI arg) to avoid exec argument size limits.
if [[ -n "${PROMPT_FILE:-}" ]] && [[ -f "$PROMPT_FILE" ]]; then
    tmux new-session -d -s claude \
        "cat '$PROMPT_FILE' | claude --dangerously-skip-permissions --print; echo \$? > /tmp/.claude-exit"
else
    tmux new-session -d -s claude \
        "claude --dangerously-skip-permissions -p \"\$PROMPT\"; echo \$? > /tmp/.claude-exit"
fi
tmux set-option -t claude history-limit 50000
touch /tmp/claude.log
tmux pipe-pane -t claude -o "cat >> /tmp/claude.log"
tail -f /tmp/claude.log &
TAIL_PID=$!
while tmux has-session -t claude 2>/dev/null; do sleep 1; done
kill $TAIL_PID 2>/dev/null || true
kill $HEARTBEAT_PID 2>/dev/null || true
wait $TAIL_PID 2>/dev/null || true
wait $HEARTBEAT_PID 2>/dev/null || true

CLAUDE_EXIT=$(cat /tmp/.claude-exit 2>/dev/null || echo 1)

sipag-state phase "finalizing" || true

if [[ "$CLAUDE_EXIT" -eq 0 ]]; then
    existing_pr=$(gh pr list --repo "${REPO}" --head "$BRANCH" \
        --state open --json number -q ".[0].number" 2>/dev/null || true)
    if [[ -z "$existing_pr" ]]; then
        echo "[sipag] Retrying PR creation after work completion"
        gh pr create --repo "${REPO}" \
                --title "$ISSUE_TITLE" \
                --body "$PR_BODY" \
                --head "$BRANCH" 2>/dev/null || true
    fi
    gh pr ready "$BRANCH" --repo "${REPO}" || true
    echo "[sipag] PR marked ready for review"

    # Update PR info in state file (may have been created in retry).
    pr_json=$(gh pr list --repo "${REPO}" --head "$BRANCH" --state open --json number,url -q '.[0]' 2>/dev/null || true)
    if [[ -n "$pr_json" ]]; then
        pr_num=$(echo "$pr_json" | jq -r '.number // empty' 2>/dev/null || true)
        pr_url=$(echo "$pr_json" | jq -r '.url // empty' 2>/dev/null || true)
        if [[ -n "$pr_num" ]]; then
            sipag-state pr "$pr_num" "$pr_url" || true
        fi
    fi

    # Only transition to needs-review if a PR actually exists.
    # Re-check for an open PR (may have been created in the retry above).
    final_pr=$(gh pr list --repo "${REPO}" --head "$BRANCH" \
        --state open --json number -q ".[0].number" 2>/dev/null || true)

    if [[ -n "$final_pr" ]]; then
        # Determine which issues were addressed by parsing the PR body for "Closes #N".
        pr_body_text=$(gh pr view "$BRANCH" --repo "${REPO}" --json body -q '.body' 2>/dev/null || true)
        addressed_issues=""
        if [[ -n "$pr_body_text" ]]; then
            # Extract all issue numbers from "Closes #N", "Fixes #N", "Resolves #N" (case-insensitive).
            addressed_issues=$(echo "$pr_body_text" | grep -ioE '(closes|fixes|resolves) #[0-9]+' | grep -oE '[0-9]+' || true)
        fi

        for issue in $ALL_ISSUES; do
            if echo "$addressed_issues" | grep -qw "$issue" 2>/dev/null; then
                # Addressed: transition in-progress → needs-review.
                transition_label_one "$issue" "in-progress" "needs-review"
            else
                # Not addressed: restore to ready for next cycle.
                transition_label_one "$issue" "in-progress" "${WORK_LABEL:-ready}"
            fi
        done
    else
        echo "[sipag] No open PR found — restoring all issues to ${WORK_LABEL:-ready}"
        transition_label "in-progress" "${WORK_LABEL:-ready}"
    fi
else
    # Failure: remove in-progress, restore work label for retry on all issues.
    transition_label "in-progress" "${WORK_LABEL:-ready}"
fi

sipag-state finish "$CLAUDE_EXIT" || true

exit "$CLAUDE_EXIT"
