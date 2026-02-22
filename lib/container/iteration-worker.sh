#!/usr/bin/env bash
set -euo pipefail

git clone "https://github.com/${REPO}.git" /work && cd /work
git config user.name "sipag"
git config user.email "sipag@localhost"
git remote set-url origin "https://x-access-token:${GH_TOKEN}@github.com/${REPO}.git"
git checkout "$BRANCH"
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
wait $TAIL_PID 2>/dev/null || true
exit "$(cat /tmp/.claude-exit 2>/dev/null || echo 1)"
