#!/usr/bin/env bats
# sipag v2 — unit tests for lib/notify.sh

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

# --- SIPAG_NOTIFY opt-out ---

@test "notify: does nothing when SIPAG_NOTIFY=0" {
  export SIPAG_NOTIFY=0
  create_mock "osascript" 0 ""
  create_mock "notify-send" 0 ""

  run notify "sipag" "test message"
  [[ "$status" -eq 0 ]]
  [[ "$(mock_call_count "osascript")" -eq 0 ]]
  [[ "$(mock_call_count "notify-send")" -eq 0 ]]
}

@test "notify: enabled by default when SIPAG_NOTIFY is unset" {
  unset SIPAG_NOTIFY
  create_mock "uname" 0 "Darwin"
  create_mock "osascript" 0 ""

  notify "sipag" "test message"
  [[ "$(mock_call_count "osascript")" -gt 0 ]]
}

@test "notify: enabled when SIPAG_NOTIFY=1" {
  export SIPAG_NOTIFY=1
  create_mock "uname" 0 "Darwin"
  create_mock "osascript" 0 ""

  notify "sipag" "test message"
  [[ "$(mock_call_count "osascript")" -gt 0 ]]
}

# --- macOS (osascript) ---

@test "notify: calls osascript on macOS" {
  create_mock "uname" 0 "Darwin"
  create_mock "osascript" 0 ""

  run notify "sipag" "test message"
  [[ "$status" -eq 0 ]]
  [[ "$(mock_call_count "osascript")" -gt 0 ]]
}

@test "notify: osascript invocation contains display notification" {
  create_mock "uname" 0 "Darwin"
  create_mock "osascript" 0 ""

  notify "sipag" "task done"
  local calls
  calls="$(get_mock_calls "osascript")"
  [[ "$calls" == *"display notification"* ]]
}

@test "notify: osascript invocation contains the message" {
  create_mock "uname" 0 "Darwin"
  create_mock "osascript" 0 ""

  notify "sipag" "my task done"
  local calls
  calls="$(get_mock_calls "osascript")"
  [[ "$calls" == *"my task done"* ]]
}

@test "notify: osascript invocation contains the title" {
  create_mock "uname" 0 "Darwin"
  create_mock "osascript" 0 ""

  notify "sipag" "my task done"
  local calls
  calls="$(get_mock_calls "osascript")"
  [[ "$calls" == *"sipag"* ]]
}

@test "notify: osascript invocation uses -e flag" {
  create_mock "uname" 0 "Darwin"
  create_mock "osascript" 0 ""

  notify "sipag" "task done"
  local calls
  calls="$(get_mock_calls "osascript")"
  [[ "$calls" == *"-e"* ]]
}

@test "notify: does not call notify-send on macOS" {
  create_mock "uname" 0 "Darwin"
  create_mock "osascript" 0 ""
  create_mock "notify-send" 0 ""

  notify "sipag" "test"
  [[ "$(mock_call_count "notify-send")" -eq 0 ]]
}

# --- Linux (notify-send) ---

@test "notify: calls notify-send on Linux when available" {
  create_mock "uname" 0 "Linux"
  create_mock "notify-send" 0 ""

  notify "sipag" "task done"
  [[ "$(mock_call_count "notify-send")" -gt 0 ]]
}

@test "notify: notify-send receives the title" {
  create_mock "uname" 0 "Linux"
  create_mock "notify-send" 0 ""

  notify "sipag" "task done"
  local calls
  calls="$(get_mock_calls "notify-send")"
  [[ "$calls" == *"sipag"* ]]
}

@test "notify: notify-send receives the message" {
  create_mock "uname" 0 "Linux"
  create_mock "notify-send" 0 ""

  notify "sipag" "task done"
  local calls
  calls="$(get_mock_calls "notify-send")"
  [[ "$calls" == *"task done"* ]]
}

@test "notify: does not call osascript on Linux" {
  create_mock "uname" 0 "Linux"
  create_mock "notify-send" 0 ""
  create_mock "osascript" 0 ""

  notify "sipag" "test"
  [[ "$(mock_call_count "osascript")" -eq 0 ]]
}

# --- Fallback (terminal bell) ---

@test "notify: succeeds with terminal bell when no notifier available" {
  # uname returns Linux but no notify-send mock → command -v notify-send fails
  create_mock "uname" 0 "Linux"

  run notify "sipag" "test message"
  [[ "$status" -eq 0 ]]
}

@test "notify: does not call osascript in fallback path" {
  create_mock "uname" 0 "Linux"
  create_mock "osascript" 0 ""

  notify "sipag" "test"
  [[ "$(mock_call_count "osascript")" -eq 0 ]]
}
