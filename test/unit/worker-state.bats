#!/usr/bin/env bats
# sipag — worker state management tests

load ../helpers/test-helpers

setup() {
  setup_common
  source "${SIPAG_ROOT}/lib/core/log.sh"
  source "${SIPAG_ROOT}/lib/core/config.sh"
  source "${SIPAG_ROOT}/lib/core/worker.sh"

  export RUN_DIR="${TEST_TMPDIR}/run"
  mkdir -p "${RUN_DIR}/workers"
}

teardown() {
  teardown_common
}

@test "_worker_write_state: produces valid JSON" {
  _worker_write_state "$RUN_DIR" "42" "running" "Fix bug" "https://github.com/o/r/issues/42" "sipag/42-fix-bug"

  local state_file="${RUN_DIR}/workers/42.json"
  [[ -f "$state_file" ]]

  # Verify it's valid JSON
  jq . "$state_file" > /dev/null
}

@test "_worker_write_state: claimed status sets correct fields" {
  _worker_write_state "$RUN_DIR" "42" "claimed" "Fix bug" "https://github.com/o/r/issues/42"

  local json
  json=$(cat "${RUN_DIR}/workers/42.json")

  assert_json_field "$json" ".task_id" "42"
  assert_json_field "$json" ".title" "Fix bug"
  assert_json_field "$json" ".status" "claimed"
  assert_json_field "$json" ".finished_at" "null"
}

@test "_worker_write_state: done status sets finished_at" {
  _worker_write_state "$RUN_DIR" "42" "claimed" "Fix bug" "https://github.com/o/r/issues/42"
  _worker_write_state "$RUN_DIR" "42" "done" "Fix bug" "https://github.com/o/r/issues/42" "sipag/42-fix-bug" "https://github.com/o/r/pull/1"

  local json
  json=$(cat "${RUN_DIR}/workers/42.json")

  assert_json_field "$json" ".status" "done"
  local finished
  finished=$(echo "$json" | jq -r '.finished_at')
  [[ "$finished" != "null" ]]
  [[ "$finished" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}T ]]
}

@test "_worker_write_state: failed status sets finished_at and error" {
  _worker_write_state "$RUN_DIR" "42" "claimed" "Fix bug" "https://github.com/o/r/issues/42"
  _worker_write_state "$RUN_DIR" "42" "failed" "Fix bug" "https://github.com/o/r/issues/42" "" "" "Clone failed"

  local json
  json=$(cat "${RUN_DIR}/workers/42.json")

  assert_json_field "$json" ".status" "failed"
  local finished
  finished=$(echo "$json" | jq -r '.finished_at')
  [[ "$finished" != "null" ]]
  local err
  err=$(echo "$json" | jq -r '.error')
  [[ "$err" == *"Clone failed"* ]]
}

@test "_worker_write_state: started_at preserved across writes" {
  _worker_write_state "$RUN_DIR" "42" "claimed" "Fix bug" "https://github.com/o/r/issues/42"

  local started1
  started1=$(jq -r '.started_at' "${RUN_DIR}/workers/42.json")

  sleep 1
  _worker_write_state "$RUN_DIR" "42" "running" "Fix bug" "https://github.com/o/r/issues/42" "sipag/42-fix-bug"

  local started2
  started2=$(jq -r '.started_at' "${RUN_DIR}/workers/42.json")

  [[ "$started1" == "$started2" ]]
}

@test "_worker_write_state: pr_url set on done" {
  _worker_write_state "$RUN_DIR" "42" "done" "Fix bug" "https://github.com/o/r/issues/42" "sipag/42-fix-bug" "https://github.com/o/r/pull/99"

  local json
  json=$(cat "${RUN_DIR}/workers/42.json")
  assert_json_field "$json" ".pr_url" "https://github.com/o/r/pull/99"
}

@test "_worker_write_state: pr_url null when not provided" {
  _worker_write_state "$RUN_DIR" "42" "running" "Fix bug" "https://github.com/o/r/issues/42" "sipag/42-fix-bug"

  local json
  json=$(cat "${RUN_DIR}/workers/42.json")
  assert_json_field "$json" ".pr_url" "null"
}

@test "_worker_write_state: special characters in error message are escaped" {
  _worker_write_state "$RUN_DIR" "42" "failed" "Fix bug" "https://github.com/o/r/issues/42" "" "" 'Error: "quote" and \backslash'

  local state_file="${RUN_DIR}/workers/42.json"
  # Should still be valid JSON
  jq . "$state_file" > /dev/null
}

@test "_worker_write_state: string task_id (ad-hoc hex ID)" {
  _worker_write_state "$RUN_DIR" "a1b2c3d4" "running" "Ad-hoc task" "adhoc://test/a1b2c3d4" "sipag/a1b2c3d4-ad-hoc"

  local state_file="${RUN_DIR}/workers/a1b2c3d4.json"
  [[ -f "$state_file" ]]
  jq . "$state_file" > /dev/null
  assert_json_field "$(cat "$state_file")" ".task_id" "a1b2c3d4"
}
