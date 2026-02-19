#!/usr/bin/env bats
# sipag — ad-hoc source plugin integration tests

load ../helpers/test-helpers
load ../helpers/mock-commands

setup() {
  setup_common
  source "${SIPAG_ROOT}/lib/core/log.sh"
  source "${SIPAG_ROOT}/lib/core/config.sh"
  source "${SIPAG_ROOT}/lib/sources/adhoc.sh"

  export SIPAG_PROJECT_SLUG="test-app"

  # Create adhoc directories
  mkdir -p "${SIPAG_HOME}/adhoc/pending"
  mkdir -p "${SIPAG_HOME}/adhoc/claimed"
  mkdir -p "${SIPAG_HOME}/adhoc/done"
}

teardown() {
  teardown_common
}

_create_task() {
  local id="$1" slug="$2" prompt="$3"
  cat >"${SIPAG_HOME}/adhoc/pending/${id}.json" <<JSONEOF
{"id":"${id}","slug":"${slug}","prompt":"${prompt}","created_at":"2026-01-01T00:00:00Z"}
JSONEOF
}

# --- source_list_tasks ---

@test "source_list_tasks: returns tasks matching project slug" {
  _create_task "abc12345" "test-app" "implement feature X"
  _create_task "def67890" "other-app" "implement feature Y"

  local result
  result=$(source_list_tasks "" "")
  [[ "$result" == *"abc12345"* ]]
  [[ "$result" != *"def67890"* ]]
}

@test "source_list_tasks: returns nothing when no pending tasks" {
  local result
  result=$(source_list_tasks "" "")
  [[ -z "$result" ]]
}

@test "source_list_tasks: returns nothing when dir doesn't exist" {
  rm -rf "${SIPAG_HOME}/adhoc"
  local result
  result=$(source_list_tasks "" "")
  [[ -z "$result" ]]
}

# --- source_claim_task ---

@test "source_claim_task: moves file from pending to claimed" {
  _create_task "abc12345" "test-app" "implement feature X"

  source_claim_task "" "abc12345" "" ""

  [[ ! -f "${SIPAG_HOME}/adhoc/pending/abc12345.json" ]]
  [[ -f "${SIPAG_HOME}/adhoc/claimed/abc12345.json" ]]
}

@test "source_claim_task: missing task → error" {
  run source_claim_task "" "nonexistent" "" ""
  [[ "$status" -ne 0 ]]
}

# --- source_get_task ---

@test "source_get_task: returns KEY=VALUE pairs" {
  _create_task "abc12345" "test-app" "implement dark mode"
  mv "${SIPAG_HOME}/adhoc/pending/abc12345.json" "${SIPAG_HOME}/adhoc/claimed/abc12345.json"

  local result
  result=$(source_get_task "" "abc12345")

  [[ "$result" == *"TASK_TITLE=adhoc: implement dark mode"* ]]
  [[ "$result" == *"TASK_BODY=implement dark mode"* ]]
  [[ "$result" == *"TASK_NUMBER=abc12345"* ]]
  [[ "$result" == *"TASK_URL=adhoc://test-app/abc12345"* ]]
}

@test "source_get_task: missing claimed task → error" {
  run source_get_task "" "nonexistent"
  [[ "$status" -ne 0 ]]
}

# --- source_complete_task ---

@test "source_complete_task: moves file to done with result" {
  _create_task "abc12345" "test-app" "implement feature"
  mv "${SIPAG_HOME}/adhoc/pending/abc12345.json" "${SIPAG_HOME}/adhoc/claimed/abc12345.json"

  source_complete_task "" "abc12345" "" "" "https://github.com/org/repo/pull/1"

  [[ ! -f "${SIPAG_HOME}/adhoc/claimed/abc12345.json" ]]
  [[ -f "${SIPAG_HOME}/adhoc/done/abc12345.json" ]]

  # Verify pr_url was added
  local pr_url
  pr_url=$(jq -r '.pr_url' "${SIPAG_HOME}/adhoc/done/abc12345.json")
  [[ "$pr_url" == "https://github.com/org/repo/pull/1" ]]
}

@test "source_complete_task: adds completed_at timestamp" {
  _create_task "abc12345" "test-app" "implement feature"
  mv "${SIPAG_HOME}/adhoc/pending/abc12345.json" "${SIPAG_HOME}/adhoc/claimed/abc12345.json"

  source_complete_task "" "abc12345" "" "" ""

  local completed_at
  completed_at=$(jq -r '.completed_at' "${SIPAG_HOME}/adhoc/done/abc12345.json")
  [[ "$completed_at" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}T ]]
}

# --- source_fail_task ---

@test "source_fail_task: moves file back to pending" {
  _create_task "abc12345" "test-app" "implement feature"
  mv "${SIPAG_HOME}/adhoc/pending/abc12345.json" "${SIPAG_HOME}/adhoc/claimed/abc12345.json"

  source_fail_task "" "abc12345" "" "" "Something went wrong"

  [[ ! -f "${SIPAG_HOME}/adhoc/claimed/abc12345.json" ]]
  [[ -f "${SIPAG_HOME}/adhoc/pending/abc12345.json" ]]
}

# --- source_comment ---

@test "source_comment: is a no-op (does not fail)" {
  run source_comment "" "abc12345" "Working on it..."
  [[ "$status" -eq 0 ]]
}
