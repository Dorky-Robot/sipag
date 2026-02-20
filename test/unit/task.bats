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

# --- task_parse_file ---

@test "parse_file: parses all frontmatter fields correctly" {
  cat >"${TEST_TMPDIR}/task.md" <<'EOF'
---
repo: salita
priority: high
source: github#142
added: 2026-02-19T22:30:00Z
---
Implement password reset flow

The user should receive an email with a one-time reset link.
Token expires after 1 hour.
EOF

  run task_parse_file "${TEST_TMPDIR}/task.md"
  [[ "$status" -eq 0 ]]

  task_parse_file "${TEST_TMPDIR}/task.md"
  [[ "$TASK_REPO" == "salita" ]]
  [[ "$TASK_PRIORITY" == "high" ]]
  [[ "$TASK_SOURCE" == "github#142" ]]
  [[ "$TASK_ADDED" == "2026-02-19T22:30:00Z" ]]
  [[ "$TASK_TITLE" == "Implement password reset flow" ]]
  [[ "$TASK_BODY" == *"The user should receive an email"* ]]
  [[ "$TASK_BODY" == *"Token expires after 1 hour."* ]]
}

@test "parse_file: defaults priority to medium when not set" {
  cat >"${TEST_TMPDIR}/task.md" <<'EOF'
---
repo: myrepo
---
Do something
EOF

  task_parse_file "${TEST_TMPDIR}/task.md"
  [[ "$TASK_PRIORITY" == "medium" ]]
  [[ "$TASK_REPO" == "myrepo" ]]
  [[ "$TASK_TITLE" == "Do something" ]]
}

@test "parse_file: handles missing optional fields (source, added)" {
  cat >"${TEST_TMPDIR}/task.md" <<'EOF'
---
repo: myrepo
priority: low
---
Simple task
EOF

  task_parse_file "${TEST_TMPDIR}/task.md"
  [[ -z "$TASK_SOURCE" ]]
  [[ -z "$TASK_ADDED" ]]
  [[ "$TASK_TITLE" == "Simple task" ]]
  [[ -z "$TASK_BODY" ]]
}

@test "parse_file: handles files with no frontmatter" {
  cat >"${TEST_TMPDIR}/task.md" <<'EOF'
Add dark mode toggle

Check the design spec before starting.
EOF

  task_parse_file "${TEST_TMPDIR}/task.md"
  [[ -z "$TASK_REPO" ]]
  [[ "$TASK_PRIORITY" == "medium" ]]
  [[ "$TASK_TITLE" == "Add dark mode toggle" ]]
  [[ "$TASK_BODY" == *"Check the design spec"* ]]
}

@test "parse_file: sets TASK_TITLE to first non-empty line after closing ---" {
  cat >"${TEST_TMPDIR}/task.md" <<'EOF'
---
repo: acme
priority: medium
---

Title comes after blank line
EOF

  task_parse_file "${TEST_TMPDIR}/task.md"
  [[ "$TASK_TITLE" == "Title comes after blank line" ]]
}

@test "parse_file: sets TASK_BODY stripping leading and trailing blank lines" {
  cat >"${TEST_TMPDIR}/task.md" <<'EOF'
---
repo: acme
---
The title

Body line one.
Body line two.

EOF

  task_parse_file "${TEST_TMPDIR}/task.md"
  [[ "$TASK_TITLE" == "The title" ]]
  [[ "$TASK_BODY" == "Body line one."$'\n'"Body line two." ]]
}

@test "parse_file: returns 1 for missing file" {
  run task_parse_file "${TEST_TMPDIR}/nonexistent.md"
  [[ "$status" -eq 1 ]]
}

# --- task_slugify ---

@test "slugify: converts title to lowercase hyphenated slug" {
  local slug
  slug=$(task_slugify "Implement Password Reset Flow")
  [[ "$slug" == "implement-password-reset-flow" ]]
}

@test "slugify: replaces special characters with hyphens" {
  local slug
  slug=$(task_slugify "Fix bug (issue #42)")
  [[ "$slug" == "fix-bug-issue-42" ]]
}

@test "slugify: squeezes consecutive non-alphanumeric into single hyphen" {
  local slug
  slug=$(task_slugify "hello   world")
  [[ "$slug" == "hello-world" ]]
}

@test "slugify: strips leading and trailing hyphens" {
  local slug
  slug=$(task_slugify "  padded title  ")
  [[ "$slug" == "padded-title" ]]
}

# --- task_next_filename ---

@test "next_filename: generates 001 for empty queue directory" {
  local qdir="${TEST_TMPDIR}/queue"
  mkdir -p "$qdir"

  local name
  name=$(task_next_filename "$qdir" "My first task")
  [[ "$name" == "001-my-first-task.md" ]]
}

@test "next_filename: increments sequence number based on existing files" {
  local qdir="${TEST_TMPDIR}/queue"
  mkdir -p "$qdir"
  touch "${qdir}/001-first-task.md"
  touch "${qdir}/002-second-task.md"

  local name
  name=$(task_next_filename "$qdir" "Third task")
  [[ "$name" == "003-third-task.md" ]]
}

@test "next_filename: works when queue directory does not exist yet" {
  local name
  name=$(task_next_filename "${TEST_TMPDIR}/queue" "New task")
  [[ "$name" == "001-new-task.md" ]]
}

@test "next_filename: zero-pads sequence numbers to three digits" {
  local qdir="${TEST_TMPDIR}/queue"
  mkdir -p "$qdir"
  touch "${qdir}/009-ninth.md"

  local name
  name=$(task_next_filename "$qdir" "Tenth task")
  [[ "$name" == "010-tenth-task.md" ]]
}
