#!/usr/bin/env bash
set -euo pipefail

# ── Label management helper ──────────────────────────────────────────────────
# The container owns label transitions so they happen atomically with the work.
# ISSUE_NUM and WORK_LABEL are passed via env vars from the host.
transition_label() {
    local remove="${1:-}" add="${2:-}"
    if [[ -z "${ISSUE_NUM:-}" ]]; then return 0; fi
    if [[ -n "$remove" ]]; then
        gh issue edit "$ISSUE_NUM" --repo "${REPO}" --remove-label "$remove" 2>/dev/null || true
    fi
    if [[ -n "$add" ]]; then
        gh issue edit "$ISSUE_NUM" --repo "${REPO}" --add-label "$add" 2>/dev/null || true
    fi
}

# ── Start: transition approved → in-progress ─────────────────────────────────
transition_label "${WORK_LABEL:-approved}" "in-progress"

sipag-state phase "cloning repo" || true

git clone "https://github.com/${REPO}.git" /work && cd /work
git config user.name "sipag"
git config user.email "sipag@localhost"
git remote set-url origin "https://x-access-token:${GH_TOKEN}@github.com/${REPO}.git"

sipag-state phase "creating branch" || true

git checkout -b "$BRANCH"
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

tmux new-session -d -s claude \
    "claude --dangerously-skip-permissions -p \"\$PROMPT\"; echo \$? > /tmp/.claude-exit"
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

    # Success: remove in-progress label (issue will be closed when PR merges via "Closes #N").
    transition_label "in-progress" ""
else
    # Failure: remove in-progress, restore work label for retry.
    transition_label "in-progress" "${WORK_LABEL:-approved}"
fi

sipag-state finish "$CLAUDE_EXIT" || true

exit "$CLAUDE_EXIT"
