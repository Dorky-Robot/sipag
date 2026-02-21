#!/usr/bin/env bats
# sipag v2 — integration tests for bin/sipag

load ../helpers/test-helpers
load ../helpers/mock-commands

setup() {
  setup_common
}

teardown() {
  teardown_common
}

# --- sipag version ---

@test "version: prints version" {
  run "${SIPAG_ROOT}/bin/sipag" version
  [[ "$status" -eq 0 ]]
  assert_output_contains "sipag 2.0.0"
}

# --- sipag help ---

@test "help: prints usage" {
  run "${SIPAG_ROOT}/bin/sipag" help
  [[ "$status" -eq 0 ]]
  assert_output_contains "autonomous dev agent"
}

@test "help: documents --once flag" {
  run "${SIPAG_ROOT}/bin/sipag" help
  [[ "$status" -eq 0 ]]
  assert_output_contains "--once"
}

@test "--once: flag is recognized (not 'Unknown flag')" {
  # --once with no repo arg will fail at the "Usage: sipag work <owner/repo>"
  # check, not at flag parsing. The important thing is it does NOT say "Unknown flag".
  run "${SIPAG_ROOT}/bin/sipag" work --once
  assert_output_not_contains "Unknown flag"
}

# --- bare sipag ---

@test "bare sipag: shows usage" {
  run "${SIPAG_ROOT}/bin/sipag"
  [[ "$status" -eq 0 ]]
  assert_output_contains "autonomous dev agent"
}
