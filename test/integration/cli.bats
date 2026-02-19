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

# --- Help & version ---

@test "sipag help: shows usage" {
  run bash "$SIPAG_CLI" help
  [[ "$status" -eq 0 ]]
  [[ "$output" == *"Usage:"* ]]
  [[ "$output" == *"sipag daemon"* ]]
  [[ "$output" == *"sipag project"* ]]
  [[ "$output" == *"sipag task"* ]]
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

# --- Daemon commands ---

@test "sipag daemon: missing subcommand → error" {
  run bash "$SIPAG_CLI" daemon
  [[ "$status" -ne 0 ]]
  [[ "$output" == *"Usage:"* ]]
}

@test "sipag daemon start: without projects exits with error" {
  run bash "$SIPAG_CLI" daemon start -f
  [[ "$status" -ne 0 ]]
  [[ "$output" == *"No projects"* ]]
}

@test "sipag daemon status: without daemon exits with error" {
  run bash "$SIPAG_CLI" daemon status
  [[ "$status" -ne 0 ]]
}

# --- Project commands ---

@test "sipag project: missing subcommand → error" {
  run bash "$SIPAG_CLI" project
  [[ "$status" -ne 0 ]]
  [[ "$output" == *"Usage:"* ]]
}

@test "sipag project add: creates project" {
  run bash "$SIPAG_CLI" project add my-app --repo=org/my-app --source=github
  [[ "$status" -eq 0 ]]
  [[ "$output" == *"registered"* ]]
  [[ -f "${SIPAG_HOME}/projects/my-app/config" ]]
}

@test "sipag project add: missing slug → error" {
  run bash "$SIPAG_CLI" project add
  [[ "$status" -ne 0 ]]
}

@test "sipag project list: shows projects" {
  # Add a project first
  bash "$SIPAG_CLI" project add test-app --repo=org/test-app --source=github
  run bash "$SIPAG_CLI" project list
  [[ "$status" -eq 0 ]]
  [[ "$output" == *"test-app"* ]]
}

@test "sipag project show: displays project config" {
  bash "$SIPAG_CLI" project add test-app --repo=org/test-app
  run bash "$SIPAG_CLI" project show test-app
  [[ "$status" -eq 0 ]]
  [[ "$output" == *"Project: test-app"* ]]
}

@test "sipag project remove: removes project" {
  bash "$SIPAG_CLI" project add test-app --repo=org/test-app
  run bash "$SIPAG_CLI" project remove test-app
  [[ "$status" -eq 0 ]]
  [[ ! -d "${SIPAG_HOME}/projects/test-app" ]]
}

# --- Task commands ---

@test "sipag task: missing subcommand → error" {
  run bash "$SIPAG_CLI" task
  [[ "$status" -ne 0 ]]
  [[ "$output" == *"Usage:"* ]]
}

@test "sipag task add: creates ad-hoc task" {
  bash "$SIPAG_CLI" project add my-app --repo=org/my-app
  run bash "$SIPAG_CLI" task add my-app "implement dark mode"
  [[ "$status" -eq 0 ]]
  [[ "$output" == *"Task"* ]]
  [[ "$output" == *"created"* ]]

  # Verify task file exists
  local count
  count=$(ls "${SIPAG_HOME}/adhoc/pending/"*.json 2>/dev/null | wc -l | tr -d ' ')
  [[ "$count" -eq 1 ]]
}

@test "sipag task add: stdin mode" {
  bash "$SIPAG_CLI" project add my-app --repo=org/my-app
  run bash -c "echo 'fix the login bug' | bash '$SIPAG_CLI' task add my-app -"
  [[ "$status" -eq 0 ]]
  [[ "$output" == *"Task"* ]]
}

@test "sipag task add: missing project → error" {
  run bash "$SIPAG_CLI" task add nonexistent "do something"
  [[ "$status" -ne 0 ]]
}

@test "sipag task list: shows tasks" {
  bash "$SIPAG_CLI" project add my-app --repo=org/my-app
  bash "$SIPAG_CLI" task add my-app "task one"
  run bash "$SIPAG_CLI" task list
  [[ "$status" -eq 0 ]]
  [[ "$output" == *"task one"* ]]
}

# --- Legacy compat ---

@test "sipag start: without config exits with error" {
  run bash "$SIPAG_CLI" start -d "$PROJECT_DIR"
  [[ "$status" -ne 0 ]]
  [[ "$output" == *".sipag"* ]]
}

@test "sipag status: without daemon or run dir shows message" {
  run bash -c "cd '$PROJECT_DIR' && SIPAG_HOME='$SIPAG_HOME' bash '$SIPAG_CLI' status ."
  [[ "$status" -ne 0 ]]
}
