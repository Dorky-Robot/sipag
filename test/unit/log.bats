#!/usr/bin/env bats
# sipag — log module tests

load ../helpers/test-helpers

setup() {
  setup_common
  source "${SIPAG_ROOT}/lib/core/log.sh"
}

teardown() {
  teardown_common
}

@test "_log_levels: debug → 0" {
  local result
  result=$(_log_levels "debug")
  [[ "$result" == "0" ]]
}

@test "_log_levels: info → 1" {
  local result
  result=$(_log_levels "info")
  [[ "$result" == "1" ]]
}

@test "_log_levels: warn → 2" {
  local result
  result=$(_log_levels "warn")
  [[ "$result" == "2" ]]
}

@test "_log_levels: error → 3" {
  local result
  result=$(_log_levels "error")
  [[ "$result" == "3" ]]
}

@test "_log_levels: unknown → 1 (default)" {
  local result
  result=$(_log_levels "garbage")
  [[ "$result" == "1" ]]
}

@test "_should_log: error message at info level → true" {
  export SIPAG_LOG_LEVEL="info"
  _should_log "error"
}

@test "_should_log: debug message at info level → false" {
  export SIPAG_LOG_LEVEL="info"
  ! _should_log "debug"
}

@test "_should_log: info message at info level → true" {
  export SIPAG_LOG_LEVEL="info"
  _should_log "info"
}

@test "log output format: timestamp + level" {
  export SIPAG_LOG_LEVEL="debug"
  local output
  output=$(log_info "test message" 2>&1)
  # Should match: YYYY-MM-DD HH:MM:SS [info ] test message
  [[ "$output" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}\ [0-9]{2}:[0-9]{2}:[0-9]{2}\ \[info\ \]\ test\ message$ ]]
}

@test "log output format: worker ID prefix" {
  export SIPAG_LOG_LEVEL="debug"
  export SIPAG_WORKER_ID="42"
  local output
  output=$(log_info "task started" 2>&1)
  [[ "$output" == *"[worker:42]"* ]]
  [[ "$output" == *"task started"* ]]
  unset SIPAG_WORKER_ID
}

@test "die: exits with code 1" {
  export SIPAG_LOG_LEVEL="error"
  run bash -c "source '${SIPAG_ROOT}/lib/core/log.sh'; die 'fatal error'"
  [[ "$status" -eq 1 ]]
}
