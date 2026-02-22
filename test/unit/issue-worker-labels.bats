#!/usr/bin/env bats
# sipag — unit tests for issue-worker.sh label management logic
#
# Tests the label management helpers from lib/container/issue-worker.sh
# in isolation (without Docker, git, or gh commands).
#
# The functions below are copied from the container script. If the script
# changes, these must be updated to match.

load ../helpers/test-helpers

setup() {
  setup_common

  export REPO="test/repo"
  export WORK_LABEL="ready"

  # Track gh issue edit calls.
  export GH_CALL_LOG="${TEST_TMPDIR}/gh-calls.log"
  cat >"${TEST_TMPDIR}/bin/gh" <<'ENDMOCK'
#!/usr/bin/env bash
echo "$@" >> "${GH_CALL_LOG}"
ENDMOCK
  chmod +x "${TEST_TMPDIR}/bin/gh"
}

teardown() {
  teardown_common
}

# ── Replicated from lib/container/issue-worker.sh ─────────────────────────────
# These must match the script exactly. Tests verify the logic works correctly.

init_all_issues() {
  ALL_ISSUES="${ISSUE_NUMS:-${ISSUE_NUM:-}}"
}

transition_label_one() {
    local issue="${1:-}" remove="${2:-}" add="${3:-}"
    if [[ -z "$issue" ]]; then return 0; fi
    if [[ -n "$remove" ]]; then
        gh issue edit "$issue" --repo "${REPO}" --remove-label "$remove" 2>/dev/null || true
    fi
    if [[ -n "$add" ]]; then
        gh issue edit "$issue" --repo "${REPO}" --add-label "$add" 2>/dev/null || true
    fi
}

transition_label() {
    local remove="${1:-}" add="${2:-}"
    for issue in $ALL_ISSUES; do
        transition_label_one "$issue" "$remove" "$add"
    done
}

# ── ALL_ISSUES resolution ─────────────────────────────────────────────────────

@test "ALL_ISSUES uses ISSUE_NUMS when set" {
  export ISSUE_NUMS="10 11 12"
  export ISSUE_NUM="99"
  init_all_issues
  [ "$ALL_ISSUES" = "10 11 12" ]
}

@test "ALL_ISSUES falls back to ISSUE_NUM when ISSUE_NUMS unset" {
  unset ISSUE_NUMS
  export ISSUE_NUM="42"
  init_all_issues
  [ "$ALL_ISSUES" = "42" ]
}

@test "ALL_ISSUES empty when both unset" {
  unset ISSUE_NUMS
  unset ISSUE_NUM
  init_all_issues
  [ -z "$ALL_ISSUES" ]
}

# ── transition_label_one ──────────────────────────────────────────────────────

@test "transition_label_one removes and adds labels for a single issue" {
  export ISSUE_NUMS="42"
  init_all_issues
  transition_label_one "42" "ready" "in-progress"

  grep -q 'issue edit 42.*--remove-label ready' "$GH_CALL_LOG"
  grep -q 'issue edit 42.*--add-label in-progress' "$GH_CALL_LOG"
}

@test "transition_label_one skips empty issue" {
  export ISSUE_NUMS="42"
  init_all_issues
  transition_label_one "" "ready" "in-progress"

  # No calls should have been made.
  [ ! -f "$GH_CALL_LOG" ] || [ ! -s "$GH_CALL_LOG" ]
}

@test "transition_label_one remove only" {
  export ISSUE_NUMS="42"
  init_all_issues
  transition_label_one "42" "ready" ""

  grep -q 'issue edit 42.*--remove-label ready' "$GH_CALL_LOG"
  # Should NOT have an --add-label call.
  run grep -- '--add-label' "$GH_CALL_LOG"
  [ "$status" -ne 0 ]
}

@test "transition_label_one add only" {
  export ISSUE_NUMS="42"
  init_all_issues
  transition_label_one "42" "" "in-progress"

  grep -q 'issue edit 42.*--add-label in-progress' "$GH_CALL_LOG"
  # Should NOT have a --remove-label call.
  run grep -- '--remove-label' "$GH_CALL_LOG"
  [ "$status" -ne 0 ]
}

# ── transition_label (all issues) ─────────────────────────────────────────────

@test "transition_label applies to all issues in ISSUE_NUMS" {
  export ISSUE_NUMS="10 11 12"
  init_all_issues
  transition_label "ready" "in-progress"

  # Each issue should get both remove and add calls.
  for num in 10 11 12; do
    grep -q "issue edit $num.*--remove-label ready" "$GH_CALL_LOG"
    grep -q "issue edit $num.*--add-label in-progress" "$GH_CALL_LOG"
  done
}

@test "transition_label with single ISSUE_NUM (backward compat)" {
  unset ISSUE_NUMS
  export ISSUE_NUM="42"
  init_all_issues
  transition_label "ready" "in-progress"

  grep -q 'issue edit 42.*--remove-label ready' "$GH_CALL_LOG"
  grep -q 'issue edit 42.*--add-label in-progress' "$GH_CALL_LOG"
}

@test "transition_label with empty ALL_ISSUES is a no-op" {
  unset ISSUE_NUMS
  unset ISSUE_NUM
  init_all_issues
  transition_label "ready" "in-progress"

  # No gh calls should have been made.
  [ ! -f "$GH_CALL_LOG" ] || [ ! -s "$GH_CALL_LOG" ]
}

@test "transition_label generates correct call count for 3 issues" {
  export ISSUE_NUMS="10 11 12"
  init_all_issues
  transition_label "ready" "in-progress"

  # 3 issues x 2 calls each (remove + add) = 6 lines.
  local count
  count=$(wc -l < "$GH_CALL_LOG" | tr -d ' ')
  [ "$count" -eq 6 ]
}
