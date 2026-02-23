#!/usr/bin/env bash
# sipag v3 worker entrypoint
#
# Environment:
#   REPO       — owner/repo (e.g. Dorky-Robot/sipag)
#   PR_NUM     — PR number to implement
#   BRANCH     — git branch to check out
#   GH_TOKEN   — GitHub token for API and clone access
#   STATE_FILE — path to state JSON file (host-mounted)
#
# The PR description is the complete assignment. This script just sets up
# the environment and runs Claude Code.

set -euo pipefail

# Report starting phase.
sipag-state phase starting

# Clone the repo and check out the PR branch.
git clone "https://x-access-token:${GH_TOKEN}@github.com/${REPO}.git" /work
cd /work
git config user.name "sipag"
git config user.email "sipag@localhost"
git fetch origin "${BRANCH}"
git checkout "${BRANCH}"

# Read PR description as the assignment.
PR_BODY=$(gh pr view "$PR_NUM" --repo "$REPO" --json body -q .body)

# Report working phase.
sipag-state phase working

# Build the prompt: PR description + worker disposition.
PROMPT="You are a sipag worker implementing a PR. The PR description below is your
complete assignment — it contains the architectural insight, approach, affected
issues, and constraints.

--- PR DESCRIPTION ---

${PR_BODY}

--- END PR DESCRIPTION ---

$(cat /prompts/worker.md 2>/dev/null || true)"

# Run Claude Code with full permissions.
EXIT_CODE=0
claude --dangerously-skip-permissions -p "$PROMPT" || EXIT_CODE=$?

# Report completion.
if [ "$EXIT_CODE" -eq 0 ]; then
    sipag-state finish "done" "$EXIT_CODE"
else
    sipag-state finish failed "$EXIT_CODE"
fi

exit "$EXIT_CODE"
