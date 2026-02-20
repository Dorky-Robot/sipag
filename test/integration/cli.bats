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
  assert_output_contains "sandbox launcher"
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

# --- sipag run ---

@test "run: requires --repo flag" {
  local dir="${TEST_TMPDIR}/fresh-sipag"
  export SIPAG_DIR="$dir"

  run "${SIPAG_ROOT}/bin/sipag" run "fix the bug"
  [[ "$status" -ne 0 ]]
  assert_output_contains "--repo"
}

@test "run: requires task description" {
  local dir="${TEST_TMPDIR}/fresh-sipag"
  export SIPAG_DIR="$dir"

  run "${SIPAG_ROOT}/bin/sipag" run --repo https://github.com/org/repo
  [[ "$status" -ne 0 ]]
  assert_output_contains "description"
}

@test "run: prints task ID and moves to done on docker success" {
  local dir="${TEST_TMPDIR}/fresh-sipag"
  export SIPAG_DIR="$dir"
  create_mock "docker" 0 "Task output"

  # Pass-through timeout
  cat >"${TEST_TMPDIR}/bin/timeout" <<'MOCK'
#!/usr/bin/env bash
shift
"$@"
MOCK
  chmod +x "${TEST_TMPDIR}/bin/timeout"

  run "${SIPAG_ROOT}/bin/sipag" run --repo https://github.com/org/repo "fix the bug"
  [[ "$status" -eq 0 ]]
  assert_output_contains "Task ID:"
  assert_output_contains "Done:"

  # File should be in done/
  local done_count
  done_count=$(ls "${dir}/done"/*.md 2>/dev/null | wc -l | tr -d ' ')
  [[ "$done_count" -gt 0 ]]
}

@test "run: moves to failed on docker failure" {
  local dir="${TEST_TMPDIR}/fresh-sipag"
  export SIPAG_DIR="$dir"
  create_mock "docker" 1 "Docker error"

  cat >"${TEST_TMPDIR}/bin/timeout" <<'MOCK'
#!/usr/bin/env bash
shift
"$@"
MOCK
  chmod +x "${TEST_TMPDIR}/bin/timeout"

  run "${SIPAG_ROOT}/bin/sipag" run --repo https://github.com/org/repo "fix the bug"
  [[ "$status" -eq 0 ]]
  assert_output_contains "Task ID:"
  assert_output_contains "Failed:"

  local failed_count
  failed_count=$(ls "${dir}/failed"/*.md 2>/dev/null | wc -l | tr -d ' ')
  [[ "$failed_count" -gt 0 ]]
}

@test "run: tracking file in done/ contains repo and started" {
  local dir="${TEST_TMPDIR}/fresh-sipag"
  export SIPAG_DIR="$dir"
  create_mock "docker" 0 ""

  cat >"${TEST_TMPDIR}/bin/timeout" <<'MOCK'
#!/usr/bin/env bash
shift
"$@"
MOCK
  chmod +x "${TEST_TMPDIR}/bin/timeout"

  "${SIPAG_ROOT}/bin/sipag" run --repo https://github.com/org/repo "fix the bug"

  local done_file
  done_file=$(ls "${dir}/done"/*.md 2>/dev/null | head -1)
  assert_file_contains "$done_file" "repo: https://github.com/org/repo"
  assert_file_contains "$done_file" "started:"
}

@test "run: tracking file in done/ contains completed and duration" {
  local dir="${TEST_TMPDIR}/fresh-sipag"
  export SIPAG_DIR="$dir"
  create_mock "docker" 0 ""

  cat >"${TEST_TMPDIR}/bin/timeout" <<'MOCK'
#!/usr/bin/env bash
shift
"$@"
MOCK
  chmod +x "${TEST_TMPDIR}/bin/timeout"

  "${SIPAG_ROOT}/bin/sipag" run --repo https://github.com/org/repo "fix the bug"

  local done_file
  done_file=$(ls "${dir}/done"/*.md 2>/dev/null | head -1)
  assert_file_contains "$done_file" "completed:"
  assert_file_contains "$done_file" "duration:"
}

@test "run: log file is created in done/ on success" {
  local dir="${TEST_TMPDIR}/fresh-sipag"
  export SIPAG_DIR="$dir"
  create_mock "docker" 0 "hello from docker"

  cat >"${TEST_TMPDIR}/bin/timeout" <<'MOCK'
#!/usr/bin/env bash
shift
"$@"
MOCK
  chmod +x "${TEST_TMPDIR}/bin/timeout"

  "${SIPAG_ROOT}/bin/sipag" run --repo https://github.com/org/repo "fix the bug"

  local done_log
  done_log=$(ls "${dir}/done"/*.log 2>/dev/null | head -1)
  assert_file_exists "$done_log"
}

@test "run: --issue flag stored in tracking file" {
  local dir="${TEST_TMPDIR}/fresh-sipag"
  export SIPAG_DIR="$dir"
  create_mock "docker" 0 ""

  cat >"${TEST_TMPDIR}/bin/timeout" <<'MOCK'
#!/usr/bin/env bash
shift
"$@"
MOCK
  chmod +x "${TEST_TMPDIR}/bin/timeout"

  "${SIPAG_ROOT}/bin/sipag" run --repo https://github.com/org/repo --issue 42 "fix the bug"

  local done_file
  done_file=$(ls "${dir}/done"/*.md 2>/dev/null | head -1)
  assert_file_contains "$done_file" "issue: 42"
}

@test "run: auto-inits directory structure" {
  local dir="${TEST_TMPDIR}/fresh-sipag"
  export SIPAG_DIR="$dir"
  create_mock "docker" 0 ""

  cat >"${TEST_TMPDIR}/bin/timeout" <<'MOCK'
#!/usr/bin/env bash
shift
"$@"
MOCK
  chmod +x "${TEST_TMPDIR}/bin/timeout"

  run "${SIPAG_ROOT}/bin/sipag" run --repo https://github.com/org/repo "fix the bug"
  [[ "$status" -eq 0 ]]
  [[ -d "${dir}/queue" ]]
  [[ -d "${dir}/running" ]]
  [[ -d "${dir}/done" ]]
  [[ -d "${dir}/failed" ]]
}

# --- sipag ps ---

@test "ps: shows running tasks with status" {
  local dir="${TEST_TMPDIR}/sipag-ps"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/queue" "${dir}/running" "${dir}/done" "${dir}/failed"

  cat >"${dir}/running/001-fix-bug.md" <<'EOF'
---
repo: https://github.com/org/repo
started: 2024-01-01T12:00:00Z
container: sipag-001-fix-bug
---
Fix the bug
EOF

  run "${SIPAG_ROOT}/bin/sipag" ps
  [[ "$status" -eq 0 ]]
  assert_output_contains "001-fix-bug"
  assert_output_contains "running"
  assert_output_contains "https://github.com/org/repo"
}

@test "ps: shows done and failed tasks" {
  local dir="${TEST_TMPDIR}/sipag-ps"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/queue" "${dir}/running" "${dir}/done" "${dir}/failed"

  cat >"${dir}/done/001-done-task.md" <<'EOF'
---
repo: https://github.com/org/repo
started: 2024-01-01T12:00:00Z
completed: 2024-01-01T12:10:00Z
---
Done task
EOF

  cat >"${dir}/failed/002-failed-task.md" <<'EOF'
---
repo: https://github.com/org/repo2
started: 2024-01-01T13:00:00Z
---
Failed task
EOF

  run "${SIPAG_ROOT}/bin/sipag" ps
  [[ "$status" -eq 0 ]]
  assert_output_contains "001-done-task"
  assert_output_contains "done"
  assert_output_contains "002-failed-task"
  assert_output_contains "failed"
}

@test "ps: shows duration for tasks with started timestamp" {
  local dir="${TEST_TMPDIR}/sipag-ps"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/running"

  cat >"${dir}/running/001-task.md" <<'EOF'
---
repo: https://github.com/org/repo
started: 2024-01-01T12:00:00Z
container: sipag-001-task
---
A task
EOF

  run "${SIPAG_ROOT}/bin/sipag" ps
  [[ "$status" -eq 0 ]]
  # Duration should not be "-" since started is set
  assert_output_contains "001-task"
}

@test "ps: shows header row" {
  local dir="${TEST_TMPDIR}/sipag-ps"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/queue" "${dir}/running" "${dir}/done" "${dir}/failed"

  run "${SIPAG_ROOT}/bin/sipag" ps
  [[ "$status" -eq 0 ]]
  assert_output_contains "ID"
  assert_output_contains "STATUS"
  assert_output_contains "REPO"
}

@test "ps: shows no tasks message when all dirs empty" {
  local dir="${TEST_TMPDIR}/sipag-ps"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/queue" "${dir}/running" "${dir}/done" "${dir}/failed"

  run "${SIPAG_ROOT}/bin/sipag" ps
  [[ "$status" -eq 0 ]]
  assert_output_contains "No tasks found"
}

# --- sipag logs ---

@test "logs: requires task id" {
  local dir="${TEST_TMPDIR}/sipag-logs"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/running"

  run "${SIPAG_ROOT}/bin/sipag" logs
  [[ "$status" -ne 0 ]]
}

@test "logs: prints log for running task" {
  local dir="${TEST_TMPDIR}/sipag-logs"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/running" "${dir}/done" "${dir}/failed"
  echo "Running task output" >"${dir}/running/001-fix-bug.log"

  run "${SIPAG_ROOT}/bin/sipag" logs 001-fix-bug
  [[ "$status" -eq 0 ]]
  assert_output_contains "Running task output"
}

@test "logs: prints log for done task" {
  local dir="${TEST_TMPDIR}/sipag-logs"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/running" "${dir}/done" "${dir}/failed"
  echo "Done task output" >"${dir}/done/001-fix-bug.log"

  run "${SIPAG_ROOT}/bin/sipag" logs 001-fix-bug
  [[ "$status" -eq 0 ]]
  assert_output_contains "Done task output"
}

@test "logs: prints log for failed task" {
  local dir="${TEST_TMPDIR}/sipag-logs"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/running" "${dir}/done" "${dir}/failed"
  echo "Error: timeout" >"${dir}/failed/001-fix-bug.log"

  run "${SIPAG_ROOT}/bin/sipag" logs 001-fix-bug
  [[ "$status" -eq 0 ]]
  assert_output_contains "Error: timeout"
}

@test "logs: errors when task not found" {
  local dir="${TEST_TMPDIR}/sipag-logs"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/running" "${dir}/done" "${dir}/failed"

  run "${SIPAG_ROOT}/bin/sipag" logs nonexistent-task
  [[ "$status" -ne 0 ]]
  assert_output_contains "Error"
  assert_output_contains "nonexistent-task"
}

# --- sipag kill ---

@test "kill: requires task id" {
  local dir="${TEST_TMPDIR}/sipag-kill"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/running"

  run "${SIPAG_ROOT}/bin/sipag" kill
  [[ "$status" -ne 0 ]]
}

@test "kill: errors when task not in running/" {
  local dir="${TEST_TMPDIR}/sipag-kill"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/running" "${dir}/failed"

  run "${SIPAG_ROOT}/bin/sipag" kill nonexistent-task
  [[ "$status" -ne 0 ]]
  assert_output_contains "Error"
  assert_output_contains "nonexistent-task"
}

@test "kill: calls docker kill and moves task to failed/" {
  local dir="${TEST_TMPDIR}/sipag-kill"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/running" "${dir}/failed"
  create_mock "docker" 0 ""

  cat >"${dir}/running/001-fix-bug.md" <<'EOF'
---
repo: https://github.com/org/repo
started: 2024-01-01T12:00:00Z
container: sipag-001-fix-bug
---
Fix the bug
EOF

  run "${SIPAG_ROOT}/bin/sipag" kill 001-fix-bug
  [[ "$status" -eq 0 ]]
  assert_output_contains "Killed"
  assert_output_contains "001-fix-bug"

  # Task moved to failed/
  [[ -f "${dir}/failed/001-fix-bug.md" ]]
  [[ ! -f "${dir}/running/001-fix-bug.md" ]]

  # docker kill was called
  local calls
  calls="$(get_mock_calls "docker")"
  [[ "$calls" == *"kill"* ]]
  [[ "$calls" == *"sipag-001-fix-bug"* ]]
}

@test "kill: moves log file to failed/ along with task" {
  local dir="${TEST_TMPDIR}/sipag-kill"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/running" "${dir}/failed"
  create_mock "docker" 0 ""

  echo "partial output" >"${dir}/running/001-fix-bug.log"
  cat >"${dir}/running/001-fix-bug.md" <<'EOF'
---
repo: https://github.com/org/repo
started: 2024-01-01T12:00:00Z
container: sipag-001-fix-bug
---
Fix the bug
EOF

  "${SIPAG_ROOT}/bin/sipag" kill 001-fix-bug

  [[ -f "${dir}/failed/001-fix-bug.log" ]]
  [[ ! -f "${dir}/running/001-fix-bug.log" ]]
}

# --- sipag stats ---

@test "stats: shows separator lines and task sections" {
  local dir="${TEST_TMPDIR}/sipag-stats"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/queue" "${dir}/running" "${dir}/done" "${dir}/failed"

  run "${SIPAG_ROOT}/bin/sipag" stats
  [[ "$status" -eq 0 ]]
  assert_output_contains "Total tasks:"
  assert_output_contains "Completed:"
  assert_output_contains "Failed:"
  assert_output_contains "Pending:"
}

@test "stats: shows all zeros when no tasks exist" {
  local dir="${TEST_TMPDIR}/sipag-stats"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/queue" "${dir}/running" "${dir}/done" "${dir}/failed"

  run "${SIPAG_ROOT}/bin/sipag" stats
  [[ "$status" -eq 0 ]]
  assert_output_contains "Total tasks:     0"
  assert_output_contains "Completed:       0"
  assert_output_contains "Failed:          0"
  assert_output_contains "Pending:         0"
}

@test "stats: counts tasks by state" {
  local dir="${TEST_TMPDIR}/sipag-stats"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/queue" "${dir}/running" "${dir}/done" "${dir}/failed"

  echo "task" >"${dir}/done/001-done.md"
  echo "task" >"${dir}/done/002-done.md"
  echo "task" >"${dir}/failed/003-failed.md"
  echo "task" >"${dir}/queue/004-pending.md"
  echo "task" >"${dir}/queue/005-pending.md"

  run "${SIPAG_ROOT}/bin/sipag" stats
  [[ "$status" -eq 0 ]]
  assert_output_contains "Total tasks:     5"
  assert_output_contains "Completed:       2"
  assert_output_contains "Failed:          1"
  assert_output_contains "Pending:         2"
}

@test "stats: shows percentage of completed and failed" {
  local dir="${TEST_TMPDIR}/sipag-stats"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/queue" "${dir}/running" "${dir}/done" "${dir}/failed"

  echo "task" >"${dir}/done/001-done.md"
  echo "task" >"${dir}/done/002-done.md"
  echo "task" >"${dir}/failed/003-failed.md"
  echo "task" >"${dir}/queue/004-pending.md"
  echo "task" >"${dir}/queue/005-pending.md"

  run "${SIPAG_ROOT}/bin/sipag" stats
  [[ "$status" -eq 0 ]]
  # 2/5 = 40%, 1/5 = 20%
  assert_output_contains "40%"
  assert_output_contains "20%"
}

@test "stats: shows duration stats from started/completed timestamps" {
  local dir="${TEST_TMPDIR}/sipag-stats"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/done" "${dir}/failed" "${dir}/queue" "${dir}/running"

  # Task 1: 10 minutes (600s)
  cat >"${dir}/done/001-task.md" <<'EOF'
---
repo: https://github.com/org/repo
started: 2024-01-01T12:00:00Z
completed: 2024-01-01T12:10:00Z
---
Task one
EOF

  # Task 2: 5 minutes (300s)
  cat >"${dir}/done/002-task.md" <<'EOF'
---
repo: https://github.com/org/repo
started: 2024-01-01T13:00:00Z
completed: 2024-01-01T13:05:00Z
---
Task two
EOF

  run "${SIPAG_ROOT}/bin/sipag" stats
  [[ "$status" -eq 0 ]]
  assert_output_contains "Avg duration:"
  assert_output_contains "Total time:"
  assert_output_contains "Longest:"
  # Longest is 10min = 600s -> "10m0s"
  assert_output_contains "10m0s"
  # 001-task is the longest
  assert_output_contains "001-task"
}

@test "stats: skips duration section when no tasks have timestamps" {
  local dir="${TEST_TMPDIR}/sipag-stats"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/done" "${dir}/failed" "${dir}/queue" "${dir}/running"

  echo "task without timestamps" >"${dir}/done/001-done.md"

  run "${SIPAG_ROOT}/bin/sipag" stats
  [[ "$status" -eq 0 ]]
  assert_output_not_contains "Avg duration:"
  assert_output_not_contains "Total time:"
  assert_output_not_contains "Longest:"
}

@test "stats: includes failed tasks in duration stats" {
  local dir="${TEST_TMPDIR}/sipag-stats"
  export SIPAG_DIR="$dir"
  mkdir -p "${dir}/done" "${dir}/failed" "${dir}/queue" "${dir}/running"

  cat >"${dir}/failed/001-failed.md" <<'EOF'
---
repo: https://github.com/org/repo
started: 2024-01-01T12:00:00Z
completed: 2024-01-01T12:03:00Z
---
Failed task
EOF

  run "${SIPAG_ROOT}/bin/sipag" stats
  [[ "$status" -eq 0 ]]
  assert_output_contains "Avg duration:"
  # 3 minutes = 180s -> "3m0s"
  assert_output_contains "3m0s"
}
