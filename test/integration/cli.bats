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

# --- sipag add --repo / --priority ---

@test "add --repo: writes task file with frontmatter to queue/" {
  local dir="${TEST_TMPDIR}/fresh-sipag"
  export SIPAG_DIR="$dir"

  run "${SIPAG_ROOT}/bin/sipag" add --repo salita --priority high "implement login"
  [[ "$status" -eq 0 ]]
  assert_output_contains "Added: implement login"

  # File should exist in queue/
  local queuefile="${dir}/queue/001-implement-login.md"
  assert_file_exists "$queuefile"
  assert_file_contains "$queuefile" "repo: salita"
  assert_file_contains "$queuefile" "priority: high"
  assert_file_contains "$queuefile" "implement login"
}

@test "add --repo: defaults priority to medium when not specified" {
  local dir="${TEST_TMPDIR}/fresh-sipag"
  export SIPAG_DIR="$dir"

  run "${SIPAG_ROOT}/bin/sipag" add --repo myrepo "fix the bug"
  [[ "$status" -eq 0 ]]

  local queuefile="${dir}/queue/001-fix-the-bug.md"
  assert_file_exists "$queuefile"
  assert_file_contains "$queuefile" "priority: medium"
}

@test "add --repo: sequences filenames correctly for multiple tasks" {
  local dir="${TEST_TMPDIR}/fresh-sipag"
  export SIPAG_DIR="$dir"

  "${SIPAG_ROOT}/bin/sipag" add --repo acme "first task"
  "${SIPAG_ROOT}/bin/sipag" add --repo acme "second task"

  assert_file_exists "${dir}/queue/001-first-task.md"
  assert_file_exists "${dir}/queue/002-second-task.md"
}

@test "add --repo: frontmatter contains added timestamp" {
  local dir="${TEST_TMPDIR}/fresh-sipag"
  export SIPAG_DIR="$dir"

  run "${SIPAG_ROOT}/bin/sipag" add --repo acme "timestamped task"
  [[ "$status" -eq 0 ]]

  local queuefile="${dir}/queue/001-timestamped-task.md"
  assert_file_contains "$queuefile" "added:"
}

@test "add --repo: flags may appear after the task text" {
  local dir="${TEST_TMPDIR}/fresh-sipag"
  export SIPAG_DIR="$dir"

  run "${SIPAG_ROOT}/bin/sipag" add "my task" --repo salita --priority low
  [[ "$status" -eq 0 ]]

  local queuefile="${dir}/queue/001-my-task.md"
  assert_file_exists "$queuefile"
  assert_file_contains "$queuefile" "repo: salita"
  assert_file_contains "$queuefile" "priority: low"
}

# --- sipag show ---

@test "show: found in done/" {
  local dir="${TEST_TMPDIR}/sipag-dirs"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/queue" "${dir}/running" "${dir}/done" "${dir}/failed"
  echo "Fix the flaky test" >"${dir}/done/003-fix-flaky-test.md"

  run "${SIPAG_ROOT}/bin/sipag" show 003-fix-flaky-test
  [[ "$status" -eq 0 ]]
  assert_output_contains "=== Task: 003-fix-flaky-test ==="
  assert_output_contains "Status: done"
  assert_output_contains "Fix the flaky test"
}

@test "show: found in failed/ with log" {
  local dir="${TEST_TMPDIR}/sipag-dirs"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/queue" "${dir}/running" "${dir}/done" "${dir}/failed"
  echo "Fix the flaky test" >"${dir}/failed/003-fix-flaky-test.md"
  echo "Error: timeout after 600s" >"${dir}/failed/003-fix-flaky-test.log"

  run "${SIPAG_ROOT}/bin/sipag" show 003-fix-flaky-test
  [[ "$status" -eq 0 ]]
  assert_output_contains "=== Task: 003-fix-flaky-test ==="
  assert_output_contains "Status: failed"
  assert_output_contains "Fix the flaky test"
  assert_output_contains "=== Log ==="
  assert_output_contains "Error: timeout after 600s"
}

@test "show: not found exits with error" {
  local dir="${TEST_TMPDIR}/sipag-dirs"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/queue" "${dir}/running" "${dir}/done" "${dir}/failed"

  run "${SIPAG_ROOT}/bin/sipag" show nonexistent-task
  [[ "$status" -ne 0 ]]
  assert_output_contains "Error"
  assert_output_contains "nonexistent-task"
}

# --- sipag retry ---

@test "retry: moves task from failed/ to queue/ and deletes log" {
  local dir="${TEST_TMPDIR}/sipag-dirs"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/queue" "${dir}/running" "${dir}/done" "${dir}/failed"
  echo "Fix the flaky test" >"${dir}/failed/003-fix-flaky-test.md"
  echo "Error: timeout after 600s" >"${dir}/failed/003-fix-flaky-test.log"

  run "${SIPAG_ROOT}/bin/sipag" retry 003-fix-flaky-test
  [[ "$status" -eq 0 ]]
  assert_output_contains "Retrying"
  assert_output_contains "003-fix-flaky-test"

  # Task file moved to queue/
  [[ -f "${dir}/queue/003-fix-flaky-test.md" ]]
  # No longer in failed/
  [[ ! -f "${dir}/failed/003-fix-flaky-test.md" ]]
  # Log deleted
  [[ ! -f "${dir}/failed/003-fix-flaky-test.log" ]]
}

@test "retry: errors if task not in failed/" {
  local dir="${TEST_TMPDIR}/sipag-dirs"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/queue" "${dir}/running" "${dir}/done" "${dir}/failed"

  run "${SIPAG_ROOT}/bin/sipag" retry nonexistent-task
  [[ "$status" -ne 0 ]]
  assert_output_contains "Error"
  assert_output_contains "nonexistent-task"
}

# --- sipag repo add ---

@test "repo add: registers a new repo" {
  local dir="${TEST_TMPDIR}/sipag-repos"
  export SIPAG_DIR="$dir"
  mkdir -p "$dir"

  run "${SIPAG_ROOT}/bin/sipag" repo add myrepo https://github.com/org/myrepo
  [[ "$status" -eq 0 ]]
  assert_output_contains "Registered"
  assert_file_contains "${dir}/repos.conf" "myrepo=https://github.com/org/myrepo"
}

@test "repo add: creates repos.conf if missing" {
  local dir="${TEST_TMPDIR}/sipag-repos"
  export SIPAG_DIR="$dir"
  mkdir -p "$dir"

  run "${SIPAG_ROOT}/bin/sipag" repo add newrepo https://github.com/org/newrepo
  [[ "$status" -eq 0 ]]
  assert_file_exists "${dir}/repos.conf"
}

@test "repo add: errors if name already exists" {
  local dir="${TEST_TMPDIR}/sipag-repos"
  export SIPAG_DIR="$dir"
  mkdir -p "$dir"
  echo "existing=https://github.com/org/existing" >"${dir}/repos.conf"

  run "${SIPAG_ROOT}/bin/sipag" repo add existing https://github.com/org/other
  [[ "$status" -ne 0 ]]
  assert_output_contains "already exists"
}

# --- sipag repo list ---

@test "repo list: prints all registered repos" {
  local dir="${TEST_TMPDIR}/sipag-repos"
  export SIPAG_DIR="$dir"
  mkdir -p "$dir"
  cat >"${dir}/repos.conf" <<'EOF'
alpha=https://github.com/org/alpha
beta=https://github.com/org/beta
EOF

  run "${SIPAG_ROOT}/bin/sipag" repo list
  [[ "$status" -eq 0 ]]
  assert_output_contains "alpha=https://github.com/org/alpha"
  assert_output_contains "beta=https://github.com/org/beta"
}

@test "repo list: shows message when no repos registered" {
  local dir="${TEST_TMPDIR}/sipag-repos"
  export SIPAG_DIR="$dir"
  mkdir -p "$dir"

  run "${SIPAG_ROOT}/bin/sipag" repo list
  [[ "$status" -eq 0 ]]
  assert_output_contains "No repos registered"
}

# --- sipag status ---

@test "status: shows items by section with counts" {
  local dir="${TEST_TMPDIR}/sipag-status"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/queue" "${dir}/running" "${dir}/done" "${dir}/failed"

  touch "${dir}/queue/005-add-input-validation"
  touch "${dir}/queue/006-refactor-date-helpers"
  touch "${dir}/running/007-fix-n-plus-one"
  touch "${dir}/done/001-password-reset"
  touch "${dir}/done/002-rate-limiting"
  touch "${dir}/failed/003-fix-flaky-test"

  run "${SIPAG_ROOT}/bin/sipag" status
  [[ "$status" -eq 0 ]]
  assert_output_contains "Queue (2):"
  assert_output_contains "005-add-input-validation"
  assert_output_contains "006-refactor-date-helpers"
  assert_output_contains "Running (1):"
  assert_output_contains "007-fix-n-plus-one"
  assert_output_contains "Done (2):"
  assert_output_contains "001-password-reset"
  assert_output_contains "002-rate-limiting"
  assert_output_contains "Failed (1):"
  assert_output_contains "003-fix-flaky-test"
}

@test "status: skips sections with 0 items" {
  local dir="${TEST_TMPDIR}/sipag-status"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/queue" "${dir}/running" "${dir}/done" "${dir}/failed"

  touch "${dir}/done/001-completed"

  run "${SIPAG_ROOT}/bin/sipag" status
  [[ "$status" -eq 0 ]]
  assert_output_contains "Done (1):"
  assert_output_not_contains "Queue"
  assert_output_not_contains "Running"
  assert_output_not_contains "Failed"
}

@test "status: shows nothing when all dirs empty" {
  local dir="${TEST_TMPDIR}/sipag-status"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/queue" "${dir}/running" "${dir}/done" "${dir}/failed"

  run "${SIPAG_ROOT}/bin/sipag" status
  [[ "$status" -eq 0 ]]
  [[ -z "$output" ]]
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
