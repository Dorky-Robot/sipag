#!/usr/bin/env bats
# sipag — tao source plugin integration tests

load ../helpers/test-helpers
load ../helpers/mock-commands

setup() {
  setup_common
  source "${SIPAG_ROOT}/lib/core/log.sh"
  source "${SIPAG_ROOT}/lib/core/config.sh"
  source "${SIPAG_ROOT}/lib/sources/tao.sh"

  # Create a test SQLite database
  export SIPAG_TAO_DB="${TEST_TMPDIR}/tao.db"
  export SIPAG_TAO_ACTION="implement-issue"

  # Only run tests if sqlite3 is available
  if ! command -v sqlite3 &>/dev/null; then
    skip "sqlite3 not available"
  fi

  sqlite3 "$SIPAG_TAO_DB" <<'SQL'
CREATE TABLE tao_suspended_actions (
  tracking_id TEXT PRIMARY KEY,
  action_name TEXT,
  prompt_text TEXT,
  stdin_data TEXT,
  status TEXT DEFAULT 'waiting_for_reply',
  archived INTEGER DEFAULT 0
);
INSERT INTO tao_suspended_actions (tracking_id, action_name, prompt_text, stdin_data, status, archived)
VALUES ('track-001', 'implement-issue', 'Fix the login bug', 'Error log here', 'waiting_for_reply', 0);
INSERT INTO tao_suspended_actions (tracking_id, action_name, prompt_text, stdin_data, status, archived)
VALUES ('track-002', 'implement-issue', 'Add dark mode', '', 'waiting_for_reply', 0);
INSERT INTO tao_suspended_actions (tracking_id, action_name, prompt_text, stdin_data, status, archived)
VALUES ('track-003', 'other-action', 'Not this one', '', 'waiting_for_reply', 0);
INSERT INTO tao_suspended_actions (tracking_id, action_name, prompt_text, stdin_data, status, archived)
VALUES ('track-004', 'implement-issue', 'Archived task', '', 'waiting_for_reply', 1);
SQL
}

teardown() {
  teardown_common
}

# --- source_list_tasks ---

@test "source_list_tasks: returns tasks matching action name" {
  local result
  result=$(source_list_tasks "" "")

  [[ "$result" == *"track-001"* ]]
  [[ "$result" == *"track-002"* ]]
  [[ "$result" != *"track-003"* ]]  # different action
  [[ "$result" != *"track-004"* ]]  # archived
}

@test "source_list_tasks: returns nothing with non-matching action" {
  export SIPAG_TAO_ACTION="nonexistent"
  local result
  result=$(source_list_tasks "" "")
  [[ -z "$result" ]]
}

@test "source_list_tasks: missing database → error" {
  export SIPAG_TAO_DB="/tmp/nonexistent.db"
  run source_list_tasks "" ""
  [[ "$status" -ne 0 ]]
}

# --- source_claim_task ---

@test "source_claim_task: sets status to paused" {
  source_claim_task "" "track-001" "" ""

  local status_val
  status_val=$(sqlite3 "$SIPAG_TAO_DB" "SELECT status FROM tao_suspended_actions WHERE tracking_id='track-001'")
  [[ "$status_val" == "paused" ]]
}

# --- source_get_task ---

@test "source_get_task: returns task details" {
  local result
  result=$(source_get_task "" "track-001")

  [[ "$result" == *"TASK_TITLE=implement-issue: Fix the login bug"* ]]
  [[ "$result" == *"TASK_BODY="* ]]
  [[ "$result" == *"TASK_NUMBER=track-001"* ]]
  [[ "$result" == *"TASK_URL=tao://track-001"* ]]
}

@test "source_get_task: includes stdin_data in body" {
  local result
  result=$(source_get_task "" "track-001")

  [[ "$result" == *"Error log here"* ]]
}

# --- source_complete_task ---

@test "source_complete_task: updates status to completed" {
  source_complete_task "" "track-001" "" "" "https://github.com/org/repo/pull/1"

  local status_val
  status_val=$(sqlite3 "$SIPAG_TAO_DB" "SELECT status FROM tao_suspended_actions WHERE tracking_id='track-001'")
  [[ "$status_val" == "completed" ]]
}

# --- source_fail_task ---

@test "source_fail_task: returns task to waiting_for_reply" {
  # First claim it
  source_claim_task "" "track-001" "" ""
  local status_val
  status_val=$(sqlite3 "$SIPAG_TAO_DB" "SELECT status FROM tao_suspended_actions WHERE tracking_id='track-001'")
  [[ "$status_val" == "paused" ]]

  # Then fail it
  source_fail_task "" "track-001" "" "" "Something went wrong"

  status_val=$(sqlite3 "$SIPAG_TAO_DB" "SELECT status FROM tao_suspended_actions WHERE tracking_id='track-001'")
  [[ "$status_val" == "waiting_for_reply" ]]
}

# --- source_comment ---

@test "source_comment: is a no-op (does not fail)" {
  run source_comment "" "track-001" "Working on it..."
  [[ "$status" -eq 0 ]]
}
