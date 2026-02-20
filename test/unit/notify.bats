#!/usr/bin/env bats
# sipag — unit tests for lib/notify.sh

load ../helpers/test-helpers
load ../helpers/mock-commands

setup() {
  setup_common
  source "${SIPAG_ROOT}/lib/notify.sh"
  unset SIPAG_NOTIFY 2>/dev/null || true
}

teardown() {
  teardown_common
}

# --- SIPAG_NOTIFY toggle ---

@test "notify: SIPAG_NOTIFY=0 disables notification" {
  export SIPAG_NOTIFY=0
  create_mock "uname" 0 "Darwin"
  create_mock "osascript" 0 ""

  notify "success" "My task"

  local count
  count="$(mock_call_count "osascript")"
  [[ "$count" -eq 0 ]]
}

@test "notify: enabled by default when SIPAG_NOTIFY is unset" {
  unset SIPAG_NOTIFY
  create_mock "uname" 0 "Darwin"
  create_mock "osascript" 0 ""

  notify "success" "My task"

  local count
  count="$(mock_call_count "osascript")"
  [[ "$count" -eq 1 ]]
}

@test "notify: SIPAG_NOTIFY=1 enables notification explicitly" {
  export SIPAG_NOTIFY=1
  create_mock "uname" 0 "Darwin"
  create_mock "osascript" 0 ""

  notify "success" "My task"

  local count
  count="$(mock_call_count "osascript")"
  [[ "$count" -eq 1 ]]
}

# --- macOS osascript ---

@test "notify: uses osascript on macOS" {
  create_mock "uname" 0 "Darwin"
  create_mock "osascript" 0 ""

  notify "success" "My task"

  local count
  count="$(mock_call_count "osascript")"
  [[ "$count" -eq 1 ]]
}

@test "notify: osascript uses display notification command" {
  create_mock "uname" 0 "Darwin"
  create_mock "osascript" 0 ""

  notify "success" "My task"

  local calls
  calls="$(get_mock_calls "osascript")"
  [[ "$calls" == *"display notification"* ]]
}

@test "notify: success message contains checkmark and PR text (macOS)" {
  create_mock "uname" 0 "Darwin"
  create_mock "osascript" 0 ""

  notify "success" "Add dark mode"

  local calls
  calls="$(get_mock_calls "osascript")"
  [[ "$calls" == *"✓"* ]]
  [[ "$calls" == *"PR ready for review"* ]]
  [[ "$calls" == *"Add dark mode"* ]]
}

@test "notify: failure message contains X mark and check logs text (macOS)" {
  create_mock "uname" 0 "Darwin"
  create_mock "osascript" 0 ""

  notify "failure" "Add dark mode"

  local calls
  calls="$(get_mock_calls "osascript")"
  [[ "$calls" == *"✗"* ]]
  [[ "$calls" == *"check logs"* ]]
  [[ "$calls" == *"Add dark mode"* ]]
}

# --- Linux notify-send ---

@test "notify: uses notify-send on Linux when available" {
  create_mock "uname" 0 "Linux"
  create_mock "notify-send" 0 ""

  notify "success" "My task"

  local count
  count="$(mock_call_count "notify-send")"
  [[ "$count" -eq 1 ]]
}

@test "notify: notify-send receives sipag title and success message" {
  create_mock "uname" 0 "Linux"
  create_mock "notify-send" 0 ""

  notify "success" "Fix the bug"

  local calls
  calls="$(get_mock_calls "notify-send")"
  [[ "$calls" == *"sipag"* ]]
  [[ "$calls" == *"✓"* ]]
  [[ "$calls" == *"Fix the bug"* ]]
  [[ "$calls" == *"PR ready for review"* ]]
}

@test "notify: notify-send receives sipag title and failure message" {
  create_mock "uname" 0 "Linux"
  create_mock "notify-send" 0 ""

  notify "failure" "Fix the bug"

  local calls
  calls="$(get_mock_calls "notify-send")"
  [[ "$calls" == *"sipag"* ]]
  [[ "$calls" == *"✗"* ]]
  [[ "$calls" == *"Fix the bug"* ]]
  [[ "$calls" == *"check logs"* ]]
}

@test "notify: falls back to terminal bell when notify-send is unavailable on Linux" {
  create_mock "uname" 0 "Linux"
  # Do not create notify-send mock — it won't be in PATH

  run notify "success" "My task"
  [[ "$status" -eq 0 ]]
}

@test "notify: does not call osascript on Linux" {
  create_mock "uname" 0 "Linux"
  create_mock "notify-send" 0 ""
  create_mock "osascript" 0 ""

  notify "success" "My task"

  local count
  count="$(mock_call_count "osascript")"
  [[ "$count" -eq 0 ]]
}
