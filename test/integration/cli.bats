#!/usr/bin/env bats
# sipag v2 — integration tests for bin/sipag

load ../helpers/test-helpers
load ../helpers/mock-commands

setup() {
  setup_common
  # Mock timeout to just run the command directly
  create_mock "timeout" 0
  export SIPAG_FILE="${TEST_TMPDIR}/tasks.md"
  export SIPAG_DIR="${TEST_TMPDIR}/sipag-dirs"
}

teardown() {
  teardown_common
}

# Helper: create a simple task file
create_tasks() {
  cat >"${SIPAG_FILE}" <<'EOF'
- [ ] First task
- [ ] Second task
- [ ] Third task
EOF
}

# --- sipag next ---

@test "next: invokes claude and marks done on success" {
  create_tasks
  create_mock "claude" 0 "Task completed"

  # We need timeout to actually run claude, not just log
  # Override timeout mock to pass through
  cat >"${TEST_TMPDIR}/bin/timeout" <<'MOCK'
#!/usr/bin/env bash
# Skip the timeout value and command name
shift
"$@"
MOCK
  chmod +x "${TEST_TMPDIR}/bin/timeout"

  run "${SIPAG_ROOT}/bin/sipag" next
  [[ "$status" -eq 0 ]]
  assert_output_contains "Task 1: First task"
  assert_output_contains "Done: First task"

  # First task should be marked done
  assert_file_contains "${SIPAG_FILE}" "- [x] First task"
  # Second should still be pending
  assert_file_contains "${SIPAG_FILE}" "- [ ] Second task"
}

@test "next: does not mark done on failure" {
  create_tasks
  create_mock "claude" 1 "Error"

  cat >"${TEST_TMPDIR}/bin/timeout" <<'MOCK'
#!/usr/bin/env bash
shift
"$@"
MOCK
  chmod +x "${TEST_TMPDIR}/bin/timeout"

  run "${SIPAG_ROOT}/bin/sipag" next
  [[ "$status" -ne 0 ]]
  assert_output_contains "Failed"

  # Task should NOT be marked done
  assert_file_contains "${SIPAG_FILE}" "- [ ] First task"
}

@test "next --continue: processes multiple tasks, stops at end" {
  cat >"${SIPAG_FILE}" <<'EOF'
- [ ] Task A
- [ ] Task B
EOF

  create_mock "claude" 0 "Done"
  cat >"${TEST_TMPDIR}/bin/timeout" <<'MOCK'
#!/usr/bin/env bash
shift
"$@"
MOCK
  chmod +x "${TEST_TMPDIR}/bin/timeout"

  run "${SIPAG_ROOT}/bin/sipag" next --continue
  [[ "$status" -eq 0 ]]
  assert_output_contains "Done: Task A"
  assert_output_contains "Done: Task B"
  assert_output_contains "No pending tasks"

  # Both marked done
  assert_file_contains "${SIPAG_FILE}" "- [x] Task A"
  assert_file_contains "${SIPAG_FILE}" "- [x] Task B"
}

@test "next --continue: stops on failure" {
  cat >"${SIPAG_FILE}" <<'EOF'
- [ ] Task A
- [ ] Task B
EOF

  # Claude fails
  create_mock "claude" 1 "Error"
  cat >"${TEST_TMPDIR}/bin/timeout" <<'MOCK'
#!/usr/bin/env bash
shift
"$@"
MOCK
  chmod +x "${TEST_TMPDIR}/bin/timeout"

  run "${SIPAG_ROOT}/bin/sipag" next --continue
  [[ "$status" -ne 0 ]]
  assert_output_contains "Failed"

  # First task not marked done, second untouched
  assert_file_contains "${SIPAG_FILE}" "- [ ] Task A"
  assert_file_contains "${SIPAG_FILE}" "- [ ] Task B"
}

@test "next --dry-run: shows task without invoking claude" {
  create_tasks
  create_mock "claude" 0 "Should not see this"

  run "${SIPAG_ROOT}/bin/sipag" next --dry-run
  [[ "$status" -eq 0 ]]
  assert_output_contains "Task 1: First task"
  assert_output_contains "dry run"

  # Claude should not have been called
  [[ "$(mock_call_count "claude")" -eq 0 ]]

  # Task should NOT be marked done
  assert_file_contains "${SIPAG_FILE}" "- [ ] First task"
}

@test "next: prints message when no tasks pending" {
  cat >"${SIPAG_FILE}" <<'EOF'
- [x] Done task
EOF

  run "${SIPAG_ROOT}/bin/sipag" next
  [[ "$status" -eq 0 ]]
  assert_output_contains "No pending tasks"
}

# --- sipag list ---

@test "list: shows task status" {
  cat >"${SIPAG_FILE}" <<'EOF'
- [x] Done task
- [ ] Pending task
EOF

  run "${SIPAG_ROOT}/bin/sipag" list
  [[ "$status" -eq 0 ]]
  assert_output_contains "[x] Done task"
  assert_output_contains "[ ] Pending task"
  assert_output_contains "1/2 done"
}

# --- sipag add ---

@test "add: appends task to file" {
  create_tasks

  run "${SIPAG_ROOT}/bin/sipag" add "New task here"
  [[ "$status" -eq 0 ]]
  assert_output_contains "Added: New task here"
  assert_file_contains "${SIPAG_FILE}" "- [ ] New task here"
}

@test "add: creates file if missing" {
  export SIPAG_FILE="${TEST_TMPDIR}/new-tasks.md"

  run "${SIPAG_ROOT}/bin/sipag" add "First task ever"
  [[ "$status" -eq 0 ]]
  assert_file_exists "${SIPAG_FILE}"
  assert_file_contains "${SIPAG_FILE}" "- [ ] First task ever"
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
  assert_output_contains "task queue feeder"
  assert_output_contains "SIPAG_FILE"
}

# --- -f flag ---

@test "-f flag: uses custom file path" {
  local custom="${TEST_TMPDIR}/custom.md"
  cat >"${custom}" <<'EOF'
- [ ] Custom task
- [x] Done custom
EOF

  run "${SIPAG_ROOT}/bin/sipag" list -f "${custom}"
  [[ "$status" -eq 0 ]]
  assert_output_contains "[ ] Custom task"
  assert_output_contains "1/2 done"
}

# --- sipag init ---

@test "init: creates queue/running/done/failed directories" {
  local dir="${TEST_TMPDIR}/fresh-sipag"
  export SIPAG_DIR="$dir"

  run "${SIPAG_ROOT}/bin/sipag" init
  [[ "$status" -eq 0 ]]
  assert_output_contains "Initialized"

  [[ -d "${dir}/queue" ]]
  [[ -d "${dir}/running" ]]
  [[ -d "${dir}/done" ]]
  [[ -d "${dir}/failed" ]]
}

@test "init: idempotent — safe to run twice" {
  local dir="${TEST_TMPDIR}/fresh-sipag"
  export SIPAG_DIR="$dir"

  "${SIPAG_ROOT}/bin/sipag" init
  run "${SIPAG_ROOT}/bin/sipag" init
  [[ "$status" -eq 0 ]]
  assert_output_contains "Already initialized"
}

@test "init: respects SIPAG_DIR env var" {
  local dir="${TEST_TMPDIR}/custom-sipag"
  export SIPAG_DIR="$dir"

  run "${SIPAG_ROOT}/bin/sipag" init
  [[ "$status" -eq 0 ]]
  [[ -d "${dir}/queue" ]]
}

# --- sipag start ---

@test "start: auto-inits directory structure" {
  local dir="${TEST_TMPDIR}/fresh-sipag"
  export SIPAG_DIR="$dir"

  run "${SIPAG_ROOT}/bin/sipag" start
  [[ "$status" -eq 0 ]]
  assert_output_contains "sipag executor starting"

  [[ -d "${dir}/queue" ]]
  [[ -d "${dir}/running" ]]
  [[ -d "${dir}/done" ]]
  [[ -d "${dir}/failed" ]]
}

# --- sipag add auto-init ---

@test "add: auto-inits directory structure on fresh install" {
  local dir="${TEST_TMPDIR}/fresh-sipag"
  export SIPAG_DIR="$dir"

  run "${SIPAG_ROOT}/bin/sipag" add "My first task"
  [[ "$status" -eq 0 ]]
  assert_output_contains "Added: My first task"

  [[ -d "${dir}/queue" ]]
  [[ -d "${dir}/running" ]]
  [[ -d "${dir}/done" ]]
  [[ -d "${dir}/failed" ]]
}

# --- default command ---

@test "bare sipag: defaults to next" {
  create_tasks
  create_mock "claude" 0 "Done"
  cat >"${TEST_TMPDIR}/bin/timeout" <<'MOCK'
#!/usr/bin/env bash
shift
"$@"
MOCK
  chmod +x "${TEST_TMPDIR}/bin/timeout"

  run "${SIPAG_ROOT}/bin/sipag"
  [[ "$status" -eq 0 ]]
  assert_output_contains "Task 1: First task"
  assert_output_contains "Done: First task"
}
