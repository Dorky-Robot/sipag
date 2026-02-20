#!/usr/bin/env bats
# sipag v2 — unit tests for lib/task.sh

load ../helpers/test-helpers

setup() {
  setup_common
  source "${SIPAG_ROOT}/lib/task.sh"
}

teardown() {
  teardown_common
}

# --- task_parse_next ---

@test "parse_next: finds first unchecked item" {
  cat >"${TEST_TMPDIR}/tasks.md" <<'EOF'
- [x] Already done
- [ ] First pending task
- [ ] Second pending task
EOF

  run task_parse_next "${TEST_TMPDIR}/tasks.md"
  [[ "$status" -eq 0 ]]

  task_parse_next "${TEST_TMPDIR}/tasks.md"
  [[ "$TASK_LINE" -eq 2 ]]
  [[ "$TASK_TITLE" == "First pending task" ]]
  [[ -z "$TASK_BODY" ]]
}

@test "parse_next: collects multi-line body" {
  cat >"${TEST_TMPDIR}/tasks.md" <<'EOF'
- [ ] Fix the signup form
  The form at /signup needs validation.
  Check email format and password strength.
- [ ] Another task
EOF

  task_parse_next "${TEST_TMPDIR}/tasks.md"
  [[ "$TASK_LINE" -eq 1 ]]
  [[ "$TASK_TITLE" == "Fix the signup form" ]]
  [[ "$TASK_BODY" == *"The form at /signup needs validation."* ]]
  [[ "$TASK_BODY" == *"Check email format and password strength."* ]]
}

@test "parse_next: skips checked items" {
  cat >"${TEST_TMPDIR}/tasks.md" <<'EOF'
- [x] Done one
- [x] Done two
- [ ] The pending one
EOF

  task_parse_next "${TEST_TMPDIR}/tasks.md"
  [[ "$TASK_LINE" -eq 3 ]]
  [[ "$TASK_TITLE" == "The pending one" ]]
}

@test "parse_next: returns 1 when all done" {
  cat >"${TEST_TMPDIR}/tasks.md" <<'EOF'
- [x] Done one
- [x] Done two
EOF

  run task_parse_next "${TEST_TMPDIR}/tasks.md"
  [[ "$status" -eq 1 ]]
}

@test "parse_next: returns 1 for missing file" {
  run task_parse_next "${TEST_TMPDIR}/nonexistent.md"
  [[ "$status" -eq 1 ]]
}

@test "parse_next: ignores headings and non-checklist lines" {
  cat >"${TEST_TMPDIR}/tasks.md" <<'EOF'
# My Tasks

Some description text.

- [x] Done task
- [ ] Pending task
EOF

  task_parse_next "${TEST_TMPDIR}/tasks.md"
  [[ "$TASK_LINE" -eq 6 ]]
  [[ "$TASK_TITLE" == "Pending task" ]]
}

# --- task_mark_done ---

@test "mark_done: marks correct line, preserves others" {
  cat >"${TEST_TMPDIR}/tasks.md" <<'EOF'
- [ ] First task
- [ ] Second task
- [ ] Third task
EOF

  task_mark_done "${TEST_TMPDIR}/tasks.md" 2

  local line2
  line2=$(sed -n '2p' "${TEST_TMPDIR}/tasks.md")
  [[ "$line2" == "- [x] Second task" ]]

  # Others unchanged
  local line1
  line1=$(sed -n '1p' "${TEST_TMPDIR}/tasks.md")
  [[ "$line1" == "- [ ] First task" ]]

  local line3
  line3=$(sed -n '3p' "${TEST_TMPDIR}/tasks.md")
  [[ "$line3" == "- [ ] Third task" ]]
}

# --- task_list ---

@test "list: counts done and pending" {
  cat >"${TEST_TMPDIR}/tasks.md" <<'EOF'
- [x] Done one
- [ ] Pending one
- [x] Done two
- [ ] Pending two
EOF

  run task_list "${TEST_TMPDIR}/tasks.md"
  [[ "$status" -eq 0 ]]
  assert_output_contains "[x] Done one"
  assert_output_contains "[ ] Pending one"
  assert_output_contains "2/4 done"
}

@test "list: returns 1 for missing file" {
  run task_list "${TEST_TMPDIR}/nonexistent.md"
  [[ "$status" -eq 1 ]]
  assert_output_contains "No task file"
}

# --- task_add ---

@test "add: creates file if missing" {
  task_add "${TEST_TMPDIR}/new.md" "Brand new task"

  assert_file_exists "${TEST_TMPDIR}/new.md"
  assert_file_contains "${TEST_TMPDIR}/new.md" "- [ ] Brand new task"
}

@test "add: appends to existing file" {
  cat >"${TEST_TMPDIR}/tasks.md" <<'EOF'
- [ ] Existing task
EOF

  task_add "${TEST_TMPDIR}/tasks.md" "Another task"

  # Both tasks present
  assert_file_contains "${TEST_TMPDIR}/tasks.md" "- [ ] Existing task"
  assert_file_contains "${TEST_TMPDIR}/tasks.md" "- [ ] Another task"
}

# --- sipag_init_dirs ---

@test "init_dirs: creates all four subdirectories" {
  local dir="${TEST_TMPDIR}/sipag"

  sipag_init_dirs "$dir"

  [[ -d "${dir}/queue" ]]
  [[ -d "${dir}/running" ]]
  [[ -d "${dir}/done" ]]
  [[ -d "${dir}/failed" ]]
}

@test "init_dirs: prints created directories and summary" {
  local dir="${TEST_TMPDIR}/sipag"

  run sipag_init_dirs "$dir"
  [[ "$status" -eq 0 ]]
  assert_output_contains "Created: ${dir}/queue"
  assert_output_contains "Created: ${dir}/running"
  assert_output_contains "Created: ${dir}/done"
  assert_output_contains "Created: ${dir}/failed"
  assert_output_contains "Initialized: ${dir}"
}

@test "init_dirs: idempotent — safe to run twice" {
  local dir="${TEST_TMPDIR}/sipag"

  sipag_init_dirs "$dir"
  run sipag_init_dirs "$dir"

  [[ "$status" -eq 0 ]]
  assert_output_contains "Already initialized: ${dir}"
}

@test "init_dirs: respects SIPAG_DIR env var when no arg given" {
  local dir="${TEST_TMPDIR}/custom-sipag"
  export SIPAG_DIR="$dir"

  sipag_init_dirs

  [[ -d "${dir}/queue" ]]
  [[ -d "${dir}/running" ]]
  [[ -d "${dir}/done" ]]
  [[ -d "${dir}/failed" ]]
}

@test "init_dirs: creates nested dirs with mkdir -p" {
  local dir="${TEST_TMPDIR}/deep/nested/sipag"

  run sipag_init_dirs "$dir"
  [[ "$status" -eq 0 ]]
  [[ -d "${dir}/queue" ]]
}
