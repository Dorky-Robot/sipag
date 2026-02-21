set -euo pipefail
git clone "https://github.com/${REPO}.git" /work && cd /work
git config user.name "sipag"
git config user.email "sipag@localhost"
git remote set-url origin "https://x-access-token:${GH_TOKEN}@github.com/${REPO}.git"
git checkout -b "$BRANCH"
git push -u origin "$BRANCH"
if gh pr create --repo "${REPO}" \
        --title "$ISSUE_TITLE" \
        --body "$PR_BODY" \
        --draft \
        --head "$BRANCH" 2>/tmp/sipag-pr-err.log; then
    echo "[sipag] Draft PR opened: branch=$BRANCH"
else
    echo "[sipag] Draft PR deferred (will retry after work): $(cat /tmp/sipag-pr-err.log)"
fi
tmux new-session -d -s claude \
    "claude --dangerously-skip-permissions -p \"\$PROMPT\"; echo \$? > /tmp/.claude-exit"
tmux set-option -t claude history-limit 50000
touch /tmp/claude.log
tmux pipe-pane -t claude -o "cat >> /tmp/claude.log"
tail -f /tmp/claude.log &
TAIL_PID=$!
while tmux has-session -t claude 2>/dev/null; do sleep 1; done
kill $TAIL_PID 2>/dev/null || true
wait $TAIL_PID 2>/dev/null || true
CLAUDE_EXIT=$(cat /tmp/.claude-exit 2>/dev/null || echo 1)
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
fi
exit "$CLAUDE_EXIT"
