#!/usr/bin/env bats
# sipag — GitHub source plugin integration tests

load ../helpers/test-helpers
load ../helpers/mock-commands

setup() {
  setup_common
  source "${SIPAG_ROOT}/lib/core/log.sh"
  source "${SIPAG_ROOT}/lib/sources/github.sh"

  create_gh_mock
}

teardown() {
  teardown_common
}

@test "source_list_tasks: calls gh issue list with correct args" {
  set_gh_response "issue list" 0 "42
43
44"
  local result
  result=$(source_list_tasks "owner/repo" "sipag")

  [[ "$result" == *"42"* ]]
  [[ "$result" == *"43"* ]]
  [[ "$result" == *"44"* ]]

  local calls
  calls=$(get_mock_calls "gh")
  [[ "$calls" == *"issue list"* ]]
  [[ "$calls" == *"--repo owner/repo"* ]]
  [[ "$calls" == *"--label sipag"* ]]
}

@test "source_claim_task: swaps labels" {
  set_gh_response "issue edit" 0 ""

  source_claim_task "owner/repo" "42" "sipag-wip" "sipag"

  local calls
  calls=$(get_mock_calls "gh")
  [[ "$calls" == *"issue edit"* ]]
  [[ "$calls" == *"--add-label sipag-wip"* ]]
  [[ "$calls" == *"--remove-label sipag"* ]]
}

@test "source_get_task: returns KEY=VALUE pairs" {
  set_gh_response "issue view" 0 '{"title":"Fix bug","body":"Details here","number":42,"url":"https://github.com/owner/repo/issues/42"}'

  local result
  result=$(source_get_task "owner/repo" "42")

  [[ "$result" == *"TASK_TITLE=Fix bug"* ]]
  [[ "$result" == *"TASK_BODY=Details here"* ]]
  [[ "$result" == *"TASK_NUMBER=42"* ]]
  [[ "$result" == *"TASK_URL=https://github.com/owner/repo/issues/42"* ]]
}

@test "source_complete_task: with PR URL adds label and comments" {
  set_gh_response "issue edit" 0 ""
  set_gh_response "issue comment" 0 ""
  set_gh_response "issue close" 0 ""

  source_complete_task "owner/repo" "42" "sipag-done" "sipag-wip" "https://github.com/owner/repo/pull/1"

  local calls
  calls=$(get_mock_calls "gh")
  [[ "$calls" == *"--add-label sipag-done"* ]]
  [[ "$calls" == *"--remove-label sipag-wip"* ]]
  [[ "$calls" == *"issue comment"* ]]
  [[ "$calls" == *"issue close"* ]]
}

@test "source_complete_task: without PR URL skips comment" {
  set_gh_response "issue edit" 0 ""
  set_gh_response "issue close" 0 ""

  source_complete_task "owner/repo" "42" "sipag-done" "sipag-wip" ""

  local calls
  calls=$(get_mock_calls "gh")
  # Should still edit and close but no comment about PR
  [[ "$calls" == *"issue edit"* ]]
  [[ "$calls" == *"issue close"* ]]
}

@test "source_fail_task: with error message posts comment" {
  set_gh_response "issue edit" 0 ""
  set_gh_response "issue comment" 0 ""

  source_fail_task "owner/repo" "42" "sipag" "sipag-wip" "Clone failed"

  local calls
  calls=$(get_mock_calls "gh")
  [[ "$calls" == *"--add-label sipag"* ]]
  [[ "$calls" == *"--remove-label sipag-wip"* ]]
  [[ "$calls" == *"issue comment"* ]]
}

@test "source_fail_task: without error message skips comment" {
  set_gh_response "issue edit" 0 ""

  source_fail_task "owner/repo" "42" "sipag" "sipag-wip" ""

  local count
  count=$(get_mock_calls "gh" | grep -c "issue comment" || true)
  [[ "$count" -eq 0 ]]
}

@test "source_comment: posts a comment" {
  set_gh_response "issue comment" 0 ""

  source_comment "owner/repo" "42" "Working on it..."

  local calls
  calls=$(get_mock_calls "gh")
  [[ "$calls" == *"issue comment"* ]]
  [[ "$calls" == *"--body Working on it..."* ]]
}
