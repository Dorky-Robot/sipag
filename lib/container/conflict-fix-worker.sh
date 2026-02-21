set -euo pipefail
git clone "https://github.com/${REPO}.git" /work && cd /work
git config user.name "sipag"
git config user.email "sipag@localhost"
git remote set-url origin "https://x-access-token:${GH_TOKEN}@github.com/${REPO}.git"
git checkout "$BRANCH"
git fetch origin main
if git merge origin/main --no-edit; then
    git push origin "$BRANCH"
    echo "[sipag] Merged main into $BRANCH (no conflicts)"
    exit 0
fi
echo "[sipag] Conflicts detected in $BRANCH, running Claude to resolve..."
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
exit "$(cat /tmp/.claude-exit 2>/dev/null || echo 1)"
