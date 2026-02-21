#!/usr/bin/env bash
# sipag — end-to-end integration test: issue → worker → PR
#
# Validates the full workflow against a real (test) GitHub repository:
#   1. Create a test issue labeled "approved"
#   2. Run "sipag work --once" to process one polling cycle
#   3. Verify a PR was opened for the issue
#   4. Clean up (close PR and issue)
#
# Prerequisites:
#   - gh auth login (or GH_TOKEN set)
#   - Docker running
#   - ANTHROPIC_API_KEY set or ~/.sipag/token present
#   - Docker image available (ghcr.io/dorky-robot/sipag-worker:latest or SIPAG_IMAGE)
#   - Test target repo exists: Dorky-Robot/sipag-test-target
#     (create once with: gh repo create Dorky-Robot/sipag-test-target --public
#      and push a minimal codebase — see README for details)
#
# Usage:
#   ./test/integration/e2e-worker.sh
#   SIPAG_IMAGE=sipag-worker:local ./test/integration/e2e-worker.sh

set -euo pipefail

SIPAG_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TEST_REPO="${SIPAG_E2E_REPO:-Dorky-Robot/sipag-test-target}"
ISSUE_NUM=""
PR_NUM=""
BRANCH_NAME=""

log() { echo "[$(date +%H:%M:%S)] $*"; }
pass() { echo "PASS: $*"; }
fail() { echo "FAIL: $*" >&2; exit 1; }

# --- Cleanup ------------------------------------------------------------------

cleanup() {
    local exit_code=$?
    echo ""
    log "=== Cleaning up ==="

    if [[ -n "$PR_NUM" ]]; then
        log "Closing PR #${PR_NUM}..."
        gh pr close "$PR_NUM" --repo "$TEST_REPO" \
            --comment "Closed by sipag integration test cleanup." 2>/dev/null || true
        # Delete the worker-created branch
        if [[ -n "$BRANCH_NAME" ]]; then
            gh api -X DELETE "repos/${TEST_REPO}/git/refs/heads/${BRANCH_NAME}" 2>/dev/null || true
        fi
    fi

    if [[ -n "$ISSUE_NUM" ]]; then
        log "Closing issue #${ISSUE_NUM}..."
        gh issue close "$ISSUE_NUM" --repo "$TEST_REPO" \
            --comment "Closed by sipag integration test cleanup." 2>/dev/null || true
    fi

    log "Cleanup done."
    exit "$exit_code"
}

trap cleanup EXIT

# --- Preflight ----------------------------------------------------------------

log "=== sipag end-to-end integration test ==="
log "Repo:  ${TEST_REPO}"
log "Image: ${SIPAG_IMAGE:-ghcr.io/dorky-robot/sipag-worker:latest}"
echo ""

log "Checking prerequisites..."

if ! command -v gh &>/dev/null; then
    fail "gh CLI not found. Install from https://cli.github.com"
fi

if ! gh auth status &>/dev/null; then
    fail "gh not authenticated. Run: gh auth login"
fi

if ! command -v docker &>/dev/null; then
    fail "docker not found"
fi

if ! docker info &>/dev/null; then
    fail "Docker daemon not running"
fi

SIPAG_DIR="${SIPAG_DIR:-$HOME/.sipag}"
if [[ ! -s "${SIPAG_DIR}/token" ]] && [[ -z "${ANTHROPIC_API_KEY:-}" ]]; then
    fail "No Claude credentials found. Set ANTHROPIC_API_KEY or populate ${SIPAG_DIR}/token"
fi

# Verify the test repo is accessible
if ! gh repo view "$TEST_REPO" &>/dev/null; then
    fail "Test repo ${TEST_REPO} not found or not accessible.
  Create it with:
    gh repo create ${TEST_REPO} --public --add-readme
  Then push a minimal codebase (e.g. a Python script or small Rust project)."
fi

pass "All prerequisites met."
echo ""

# --- Step 1: Create test issue ------------------------------------------------

log "=== Step 1: Creating test issue ==="

TIMESTAMP="$(date +%Y%m%d-%H%M%S)"
ISSUE_TITLE="Test: add hello function [${TIMESTAMP}]"
ISSUE_BODY="Add a \`hello()\` function to \`main.py\` that returns the string \`'hello world'\`.

This is an automated sipag integration test created at ${TIMESTAMP}. It validates
the full issue → worker → PR workflow. The PR will be closed automatically after
the test completes."

ISSUE_NUM=$(gh issue create \
    --repo "$TEST_REPO" \
    --title "$ISSUE_TITLE" \
    --body "$ISSUE_BODY" \
    --label "approved" \
    --json number -q '.number')

log "Created issue #${ISSUE_NUM}: ${ISSUE_TITLE}"
echo ""

# --- Step 2: Run sipag work --once --------------------------------------------

log "=== Step 2: Running sipag work --once ==="
log "This will process one polling cycle and exit."
log "(Expected: worker picks up issue #${ISSUE_NUM}, opens a PR)"
echo ""

WORKER_BATCH_SIZE=1 "${SIPAG_ROOT}/bin/sipag" work "$TEST_REPO" --once
echo ""

# --- Step 3: Verify a PR was created ------------------------------------------

log "=== Step 3: Verifying PR was created for issue #${ISSUE_NUM} ==="

# Allow a brief moment for GitHub API to reflect the new PR
sleep 5

# Look for an open PR whose body references the test issue
PR_JSON=$(gh pr list \
    --repo "$TEST_REPO" \
    --state open \
    --search "closes #${ISSUE_NUM}" \
    --json number,title,url,headRefName 2>/dev/null || echo "[]")

PR_NUM=$(echo "$PR_JSON" | jq -r \
    --argjson issue "$ISSUE_NUM" \
    '.[] | select(.title | test("(?i)test")) | .number' \
    2>/dev/null | head -1 || true)

# Fallback: find any open PR referencing the issue via body content
if [[ -z "$PR_NUM" ]]; then
    PR_NUM=$(gh pr list \
        --repo "$TEST_REPO" \
        --state all \
        --json number,body,headRefName \
        --jq ".[] | select(.body // \"\" | test(\"(closes|fixes|resolves) #${ISSUE_NUM}\\\\b\")) | .number" \
        2>/dev/null | head -1 || true)
fi

if [[ -z "$PR_NUM" ]]; then
    fail "No PR found referencing issue #${ISSUE_NUM}.
  Check worker logs in /tmp/sipag-backlog/issue-${ISSUE_NUM}.log for details."
fi

BRANCH_NAME=$(gh pr view "$PR_NUM" --repo "$TEST_REPO" --json headRefName -q '.headRefName' 2>/dev/null || true)
PR_URL=$(gh pr view "$PR_NUM" --repo "$TEST_REPO" --json url -q '.url' 2>/dev/null || true)

pass "PR #${PR_NUM} was created for issue #${ISSUE_NUM}"
log "  URL:    ${PR_URL}"
log "  Branch: ${BRANCH_NAME}"
echo ""

# --- Done (cleanup runs via trap) --------------------------------------------

log "=== Integration test PASSED ==="
