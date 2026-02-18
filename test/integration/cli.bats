#!/usr/bin/env bats
# sipag — CLI integration tests

load ../helpers/test-helpers

setup() {
  setup_common
  export SIPAG_CLI="${SIPAG_ROOT}/bin/sipag"
}

teardown() {
  teardown_common
}

@test "sipag help: shows usage" {
  run bash "$SIPAG_CLI" help
  [[ "$status" -eq 0 ]]
  [[ "$output" == *"Usage:"* ]]
  [[ "$output" == *"sipag init"* ]]
  [[ "$output" == *"sipag start"* ]]
}

@test "sipag --help: shows usage" {
  run bash "$SIPAG_CLI" --help
  [[ "$status" -eq 0 ]]
  [[ "$output" == *"Usage:"* ]]
}

@test "sipag -h: shows usage" {
  run bash "$SIPAG_CLI" -h
  [[ "$status" -eq 0 ]]
  [[ "$output" == *"Usage:"* ]]
}

@test "sipag version: returns version string" {
  run bash "$SIPAG_CLI" version
  [[ "$status" -eq 0 ]]
  [[ "$output" == "sipag v"* ]]
}

@test "sipag (no args): shows help" {
  run bash "$SIPAG_CLI"
  [[ "$status" -eq 0 ]]
  [[ "$output" == *"Usage:"* ]]
}

@test "unknown command: shows help + exits non-zero" {
  run bash "$SIPAG_CLI" foobar
  [[ "$status" -ne 0 ]]
  [[ "$output" == *"Unknown command: foobar"* ]]
  [[ "$output" == *"Usage:"* ]]
}

@test "sipag start: without config exits with error" {
  run bash "$SIPAG_CLI" start -d "$PROJECT_DIR"
  [[ "$status" -ne 0 ]]
  [[ "$output" == *".sipag"* ]]
}

@test "sipag status: without run dir exits with error" {
  # Create a config so config_load succeeds, but no run dir
  create_test_config "$PROJECT_DIR"
  run bash -c "cd '$PROJECT_DIR' && bash '$SIPAG_CLI' status ."
  [[ "$status" -ne 0 ]]
}
