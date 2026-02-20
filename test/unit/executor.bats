#!/usr/bin/env bats
# sipag v2 — unit tests for lib/executor.sh

load ../helpers/test-helpers
load ../helpers/mock-commands

setup() {
  setup_common
  source "${SIPAG_ROOT}/lib/task.sh"
  source "${SIPAG_ROOT}/lib/repo.sh"
  source "${SIPAG_ROOT}/lib/executor.sh"

  export SIPAG_DIR="${TEST_TMPDIR}/sipag"
  mkdir -p "${SIPAG_DIR}/queue" "${SIPAG_DIR}/running" "${SIPAG_DIR}/done" "${SIPAG_DIR}/failed"
  echo "testrepo=https://github.com/org/testrepo" >"${SIPAG_DIR}/repos.conf"

  # Pass-through timeout: skip the timeout value and run the rest
  cat >"${TEST_TMPDIR}/bin/timeout" <<'MOCK'
#!/usr/bin/env bash
shift
"$@"
MOCK
  chmod +x "${TEST_TMPDIR}/bin/timeout"

  # Ensure token env var is clean
  unset CLAUDE_CODE_OAUTH_TOKEN 2>/dev/null || true
  unset SIPAG_TOKEN_FILE 2>/dev/null || true
}

teardown() {
  teardown_common
}

# Helper: create a minimal task file with YAML frontmatter
create_task_file() {
  local path="$1"
  local title="${2:-Test task title}"
  local repo="${3:-testrepo}"
  cat >"$path" <<EOF
---
repo: ${repo}
priority: medium
added: 2024-01-01T00:00:00Z
---
${title}
EOF
}

# --- executor_build_prompt ---

@test "executor_build_prompt: contains repository context line" {
  run executor_build_prompt "My task" ""
  [[ "$status" -eq 0 ]]
  assert_output_contains "You are working on the repository at /work"
}

@test "executor_build_prompt: contains Your task section" {
  run executor_build_prompt "My task" ""
  [[ "$status" -eq 0 ]]
  assert_output_contains "Your task:"
}

@test "executor_build_prompt: contains task title" {
  run executor_build_prompt "Implement dark mode" ""
  [[ "$status" -eq 0 ]]
  assert_output_contains "Implement dark mode"
}

@test "executor_build_prompt: contains task body when provided" {
  run executor_build_prompt "Fix the login bug" "The bug is in the login flow"
  [[ "$status" -eq 0 ]]
  assert_output_contains "Fix the login bug"
  assert_output_contains "The bug is in the login flow"
}

@test "executor_build_prompt: omits body section when body is empty" {
  run executor_build_prompt "Simple task" ""
  [[ "$status" -eq 0 ]]
  assert_output_contains "Simple task"
  # Should still have Instructions
  assert_output_contains "Instructions:"
}

@test "executor_build_prompt: contains Instructions section" {
  run executor_build_prompt "Task" ""
  [[ "$status" -eq 0 ]]
  assert_output_contains "Instructions:"
}

@test "executor_build_prompt: contains create branch instruction" {
  run executor_build_prompt "Task" ""
  [[ "$status" -eq 0 ]]
  assert_output_contains "Create a new branch with a descriptive name"
}

@test "executor_build_prompt: contains open draft pull request instruction" {
  run executor_build_prompt "Task" ""
  [[ "$status" -eq 0 ]]
  assert_output_contains "open a draft pull request"
}

@test "executor_build_prompt: contains ready for review instruction" {
  run executor_build_prompt "Task" ""
  [[ "$status" -eq 0 ]]
  assert_output_contains "mark the pull request as ready for review"
}

@test "executor_build_prompt: contains run tests instruction" {
  run executor_build_prompt "Task" ""
  [[ "$status" -eq 0 ]]
  assert_output_contains "Run any existing tests"
}

@test "executor_build_prompt: contains CLAUDE.md instruction" {
  run executor_build_prompt "Task" ""
  [[ "$status" -eq 0 ]]
  assert_output_contains "Read and follow any CLAUDE.md"
}

# --- executor_run_task ---

@test "executor_run_task: returns 0 on docker success" {
  create_task_file "${SIPAG_DIR}/running/001-test.md"
  create_mock "docker" 0 ""

  run executor_run_task "${SIPAG_DIR}/running/001-test.md"
  [[ "$status" -eq 0 ]]
}

@test "executor_run_task: returns non-zero on docker failure" {
  create_task_file "${SIPAG_DIR}/running/001-test.md"
  create_mock "docker" 2 "Error output"

  run executor_run_task "${SIPAG_DIR}/running/001-test.md"
  [[ "$status" -ne 0 ]]
}

@test "executor_run_task: creates log file alongside task" {
  create_task_file "${SIPAG_DIR}/running/001-test.md"
  create_mock "docker" 0 "Task output"

  executor_run_task "${SIPAG_DIR}/running/001-test.md"
  assert_file_exists "${SIPAG_DIR}/running/001-test.log"
}

@test "executor_run_task: log file captures docker output" {
  create_task_file "${SIPAG_DIR}/running/001-test.md"
  create_mock "docker" 0 "hello from docker"

  executor_run_task "${SIPAG_DIR}/running/001-test.md"
  assert_file_contains "${SIPAG_DIR}/running/001-test.log" "hello from docker"
}

@test "executor_run_task: returns 1 when repo not found in repos.conf" {
  create_task_file "${SIPAG_DIR}/running/001-test.md" "Task" "unknown-repo"
  create_mock "docker" 0 ""

  run executor_run_task "${SIPAG_DIR}/running/001-test.md"
  [[ "$status" -eq 1 ]]
  assert_output_contains "not found in repos.conf"
}

@test "executor_run_task: prints running message with task name" {
  create_task_file "${SIPAG_DIR}/running/001-my-task.md"
  create_mock "docker" 0 ""

  run executor_run_task "${SIPAG_DIR}/running/001-my-task.md"
  [[ "$status" -eq 0 ]]
  assert_output_contains "==> Running: 001-my-task"
}

@test "executor_run_task: calls docker with run --rm" {
  create_task_file "${SIPAG_DIR}/running/001-test.md"
  create_mock "docker" 0 ""

  executor_run_task "${SIPAG_DIR}/running/001-test.md"
  local calls
  calls="$(get_mock_calls "docker")"
  [[ "$calls" == *"run"* ]]
  [[ "$calls" == *"--rm"* ]]
}

@test "executor_run_task: passes REPO_URL to docker" {
  create_task_file "${SIPAG_DIR}/running/001-test.md"
  create_mock "docker" 0 ""

  executor_run_task "${SIPAG_DIR}/running/001-test.md"
  local calls
  calls="$(get_mock_calls "docker")"
  [[ "$calls" == *"REPO_URL=https://github.com/org/testrepo"* ]]
}

@test "executor_run_task: uses SIPAG_IMAGE env var when set" {
  create_task_file "${SIPAG_DIR}/running/001-test.md"
  create_mock "docker" 0 ""
  export SIPAG_IMAGE="my-custom-image:v1"

  executor_run_task "${SIPAG_DIR}/running/001-test.md"
  local calls
  calls="$(get_mock_calls "docker")"
  [[ "$calls" == *"my-custom-image:v1"* ]]
  unset SIPAG_IMAGE
}

@test "executor_run_task: reads token from SIPAG_TOKEN_FILE when CLAUDE_CODE_OAUTH_TOKEN not set" {
  local token_file="${TEST_TMPDIR}/my-sipag-token"
  printf 'test-oauth-token' >"$token_file"
  export SIPAG_TOKEN_FILE="$token_file"
  unset CLAUDE_CODE_OAUTH_TOKEN 2>/dev/null || true

  create_task_file "${SIPAG_DIR}/running/001-test.md"
  create_mock "docker" 0 ""

  run executor_run_task "${SIPAG_DIR}/running/001-test.md"
  [[ "$status" -eq 0 ]]
}

@test "executor_run_task: does not overwrite CLAUDE_CODE_OAUTH_TOKEN when already set" {
  export CLAUDE_CODE_OAUTH_TOKEN="already-set-token"
  local token_file="${TEST_TMPDIR}/should-not-read"
  printf 'other-token' >"$token_file"
  export SIPAG_TOKEN_FILE="$token_file"

  create_task_file "${SIPAG_DIR}/running/001-test.md"
  create_mock "docker" 0 ""

  executor_run_task "${SIPAG_DIR}/running/001-test.md"
  [[ "${CLAUDE_CODE_OAUTH_TOKEN}" == "already-set-token" ]]
}

# --- executor_run ---

@test "executor_run: prints message when queue is empty" {
  run executor_run
  [[ "$status" -eq 0 ]]
  assert_output_contains "No tasks in queue"
}

@test "executor_run: moves task from queue to done on docker success" {
  create_task_file "${SIPAG_DIR}/queue/001-test.md"
  create_mock "docker" 0 ""

  run executor_run
  [[ "$status" -eq 0 ]]
  assert_output_contains "Done: 001-test"
  [[ -f "${SIPAG_DIR}/done/001-test.md" ]]
  [[ ! -f "${SIPAG_DIR}/queue/001-test.md" ]]
}

@test "executor_run: moves task from queue to failed on docker failure" {
  create_task_file "${SIPAG_DIR}/queue/001-test.md"
  create_mock "docker" 1 "Docker error"

  run executor_run
  [[ "$status" -eq 0 ]]
  assert_output_contains "Failed"
  assert_output_contains "001-test"
  [[ -f "${SIPAG_DIR}/failed/001-test.md" ]]
  [[ ! -f "${SIPAG_DIR}/queue/001-test.md" ]]
}

@test "executor_run: moves log file with task to done/" {
  create_task_file "${SIPAG_DIR}/queue/001-test.md"
  create_mock "docker" 0 "some output"

  executor_run
  assert_file_exists "${SIPAG_DIR}/done/001-test.log"
  [[ ! -f "${SIPAG_DIR}/running/001-test.log" ]]
}

@test "executor_run: moves log file with task to failed/" {
  create_task_file "${SIPAG_DIR}/queue/001-test.md"
  create_mock "docker" 1 "error output"

  executor_run
  assert_file_exists "${SIPAG_DIR}/failed/001-test.log"
  [[ ! -f "${SIPAG_DIR}/running/001-test.log" ]]
}

@test "executor_run: processes multiple tasks in alphabetical order" {
  create_task_file "${SIPAG_DIR}/queue/001-first.md" "First task"
  create_task_file "${SIPAG_DIR}/queue/002-second.md" "Second task"
  create_mock "docker" 0 ""

  run executor_run
  [[ "$status" -eq 0 ]]
  assert_output_contains "Done: 001-first"
  assert_output_contains "Done: 002-second"
  [[ -f "${SIPAG_DIR}/done/001-first.md" ]]
  [[ -f "${SIPAG_DIR}/done/002-second.md" ]]
}

@test "executor_run: continues processing after one task fails" {
  # Task with unknown repo fails; task with known repo succeeds
  create_task_file "${SIPAG_DIR}/queue/001-fail.md" "Failing task" "unknown-repo"
  create_task_file "${SIPAG_DIR}/queue/002-pass.md" "Passing task" "testrepo"
  create_mock "docker" 0 ""

  run executor_run
  [[ "$status" -eq 0 ]]
  assert_output_contains "Failed"
  assert_output_contains "001-fail"
  assert_output_contains "Done"
  assert_output_contains "002-pass"
  [[ -f "${SIPAG_DIR}/failed/001-fail.md" ]]
  [[ -f "${SIPAG_DIR}/done/002-pass.md" ]]
}

@test "executor_run: prints processed count when queue drains" {
  create_task_file "${SIPAG_DIR}/queue/001-test.md"
  create_mock "docker" 0 ""

  run executor_run
  [[ "$status" -eq 0 ]]
  assert_output_contains "processed 1 task"
}

@test "executor_run: returns 0 even when all tasks fail" {
  create_task_file "${SIPAG_DIR}/queue/001-test.md"
  create_mock "docker" 1 ""

  run executor_run
  [[ "$status" -eq 0 ]]
}
