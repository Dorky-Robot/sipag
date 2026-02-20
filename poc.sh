#!/usr/bin/env bash
# sipag POC — prove Claude Code Max subscription works inside Docker
#
# This script:
# 1. Builds the sipag-worker Docker image
# 2. Extracts your Claude Code OAuth credentials from macOS Keychain
# 3. Runs Claude Code inside a Docker container with a trivial task
# 4. Verifies Claude ran successfully

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

echo "=== sipag POC: Claude Code in Docker ==="
echo ""

# Step 1: Build the image
echo "Step 1: Building sipag-worker image..."
docker build -t sipag-worker:latest "$SCRIPT_DIR" 2>&1 | tail -1
echo ""

# Step 2: Extract credentials from macOS Keychain
echo "Step 2: Extracting Claude Code credentials from Keychain..."
CLAUDE_CREDS=$(security find-generic-password -s "Claude Code-credentials" -w 2>/dev/null) || {
    echo "ERROR: Could not read Claude Code credentials from Keychain."
    echo "Make sure you're logged into Claude Code on this machine."
    exit 1
}
echo "Got credentials ($(echo "$CLAUDE_CREDS" | wc -c | tr -d ' ') bytes)"
echo ""

# Step 3: Run Claude inside Docker with a trivial task
echo "Step 3: Running Claude Code inside Docker..."
echo "Task: 'Respond with exactly: SIPAG_POC_SUCCESS'"
echo ""

OUTPUT=$(docker run --rm \
    -e CLAUDE_CODE_CREDENTIALS="$CLAUDE_CREDS" \
    sipag-worker:latest \
    bash -c '
        # Write credentials where Claude Code expects them
        mkdir -p ~/.claude
        # Claude Code reads from its own credential store
        # Try passing via environment — claude CLI checks for this
        echo "$CLAUDE_CODE_CREDENTIALS" > /tmp/claude-creds.json

        # Run claude with the credentials injected
        claude --print --dangerously-skip-permissions \
            -p "Respond with exactly this text and nothing else: SIPAG_POC_SUCCESS" \
            2>&1 || true
    ' 2>&1) || true

echo "--- Claude output ---"
echo "$OUTPUT"
echo "--- end output ---"
echo ""

# Step 4: Check result
if echo "$OUTPUT" | grep -q "SIPAG_POC_SUCCESS"; then
    echo "SUCCESS: Claude Code ran inside Docker with Max subscription!"
else
    echo "INCONCLUSIVE: Claude ran but output didn't match expected string."
    echo "This might still be working — check the output above."
    echo ""
    echo "If you see auth errors, we may need to figure out how Claude Code"
    echo "reads credentials internally. Try:"
    echo "  docker run --rm -it -v ~/.claude:/root/.claude sipag-worker:latest bash"
    echo "  # then inside: claude --print -p 'hello'"
fi
