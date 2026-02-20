#!/usr/bin/env bats
# sipag — unit tests for lib/review.sh

load ../helpers/test-helpers
load ../helpers/mock-commands

setup() {
  setup_common
  source "${SIPAG_ROOT}/lib/review.sh"
}

teardown() {
  teardown_common
}

# ─── Argument validation ──────────────────────────────────────────────────────

@test "review_run: missing repo argument prints usage and fails" {
  run review_run
  [ "$status" -ne 0 ]
  [[ "$output" == *"Usage"* ]]
}

@test "review_run: empty repo string prints usage and fails" {
  run review_run ""
  [ "$status" -ne 0 ]
  [[ "$output" == *"Usage"* ]]
}

@test "review_run: repo without slash prints error and fails" {
  run review_run "notarepo"
  [ "$status" -ne 0 ]
  [[ "$output" == *"owner/repo"* ]]
}

# ─── No open PRs ─────────────────────────────────────────────────────────────

@test "review_run: no open PRs prints message and succeeds" {
  create_mock "gh" 0 ""
  run review_run "owner/repo"
  [ "$status" -eq 0 ]
  [[ "$output" == *"No open pull requests"* ]]
}

# ─── gh pr list is called correctly ──────────────────────────────────────────

@test "review_run: calls gh pr list with correct flags" {
  create_mock "gh" 0 ""
  review_run "owner/repo"
  local calls
  calls="$(get_mock_calls "gh")"
  [[ "$calls" == *"pr list"* ]]
  [[ "$calls" == *"--repo owner/repo"* ]]
  [[ "$calls" == *"--state open"* ]]
}

# ─── gh failure ───────────────────────────────────────────────────────────────

@test "review_run: fails when gh pr list returns non-zero" {
  create_mock "gh" 1 "error: no such repo"
  run review_run "owner/repo"
  [ "$status" -ne 0 ]
  [[ "$output" == *"failed to list PRs"* ]]
}

# ─── Approve verdict ──────────────────────────────────────────────────────────

@test "review_run: approves PR when Claude returns approve verdict" {
  # gh: first call (pr list) returns PR 42; subsequent calls succeed
  local gh_responses=("42" '{"title":"Fix bug","body":"","files":[],"comments":[]}' "" "")
  local call_num=0
  local mock_bin="${TEST_TMPDIR}/bin/gh"
  local diff_file="${TEST_TMPDIR}/gh-diff.txt"
  local pr_list_log="${TEST_TMPDIR}/mock-calls-gh"
  echo "dummy diff" > "$diff_file"

  # Create a stateful gh mock that returns different values per call
  cat > "$mock_bin" <<ENDMOCK
#!/usr/bin/env bash
printf '%s\n' "\$*" >> "${TEST_TMPDIR}/mock-calls-gh"
call_num_file="${TEST_TMPDIR}/gh-call-num"
n=\$(cat "\$call_num_file" 2>/dev/null || echo 0)
n=\$((n + 1))
echo "\$n" > "\$call_num_file"
case "\$n" in
  1) printf '42\n' ;;
  2) printf '{"title":"Fix bug","body":"","files":[],"comments":[]}\n' ;;
  3) printf 'diff --git a/foo.c b/foo.c\n+int x = 1;\n' ;;
  *) exit 0 ;;
esac
ENDMOCK
  chmod +x "$mock_bin"

  # claude mock returns approve JSON
  local claude_bin="${TEST_TMPDIR}/bin/claude"
  cat > "$claude_bin" <<'ENDCLAUDE'
#!/usr/bin/env bash
printf '%s\n' '{"verdict":"approve","summary":"LGTM","body":"Looks good to me."}'
ENDCLAUDE
  chmod +x "$claude_bin"

  run review_run "owner/repo"
  [ "$status" -eq 0 ]
  [[ "$output" == *"approve"* ]]
  # Verify gh pr review --approve was called
  local calls
  calls="$(get_mock_calls "gh")"
  [[ "$calls" == *"--approve"* ]]
}

@test "review_run: requests changes when Claude returns request_changes verdict" {
  local mock_bin="${TEST_TMPDIR}/bin/gh"
  cat > "$mock_bin" <<ENDMOCK
#!/usr/bin/env bash
printf '%s\n' "\$*" >> "${TEST_TMPDIR}/mock-calls-gh"
call_num_file="${TEST_TMPDIR}/gh-call-num"
n=\$(cat "\$call_num_file" 2>/dev/null || echo 0)
n=\$((n + 1))
echo "\$n" > "\$call_num_file"
case "\$n" in
  1) printf '7\n' ;;
  2) printf '{"title":"Add feature","body":"","files":[],"comments":[]}\n' ;;
  3) printf 'diff --git a/main.c b/main.c\n' ;;
  *) exit 0 ;;
esac
ENDMOCK
  chmod +x "$mock_bin"

  local claude_bin="${TEST_TMPDIR}/bin/claude"
  cat > "$claude_bin" <<'ENDCLAUDE'
#!/usr/bin/env bash
printf '%s\n' '{"verdict":"request_changes","summary":"Missing tests","body":"Please add unit tests."}'
ENDCLAUDE
  chmod +x "$claude_bin"

  run review_run "owner/repo"
  [ "$status" -eq 0 ]
  [[ "$output" == *"request_changes"* ]]
  local calls
  calls="$(get_mock_calls "gh")"
  [[ "$calls" == *"--request-changes"* ]]
}

@test "review_run: comments when Claude returns comment verdict" {
  local mock_bin="${TEST_TMPDIR}/bin/gh"
  cat > "$mock_bin" <<ENDMOCK
#!/usr/bin/env bash
printf '%s\n' "\$*" >> "${TEST_TMPDIR}/mock-calls-gh"
call_num_file="${TEST_TMPDIR}/gh-call-num"
n=\$(cat "\$call_num_file" 2>/dev/null || echo 0)
n=\$((n + 1))
echo "\$n" > "\$call_num_file"
case "\$n" in
  1) printf '3\n' ;;
  2) printf '{"title":"Refactor","body":"","files":[],"comments":[]}\n' ;;
  3) printf 'diff --git a/foo.c b/foo.c\n' ;;
  *) exit 0 ;;
esac
ENDMOCK
  chmod +x "$mock_bin"

  local claude_bin="${TEST_TMPDIR}/bin/claude"
  cat > "$claude_bin" <<'ENDCLAUDE'
#!/usr/bin/env bash
printf '%s\n' '{"verdict":"comment","summary":"Looks fine","body":"Minor style note."}'
ENDCLAUDE
  chmod +x "$claude_bin"

  run review_run "owner/repo"
  [ "$status" -eq 0 ]
  [[ "$output" == *"comment"* ]]
  local calls
  calls="$(get_mock_calls "gh")"
  [[ "$calls" == *"--comment"* ]]
}

# ─── Claude parse failure ─────────────────────────────────────────────────────

@test "review_run: skips PR and warns when Claude returns unparseable output" {
  local mock_bin="${TEST_TMPDIR}/bin/gh"
  cat > "$mock_bin" <<ENDMOCK
#!/usr/bin/env bash
printf '%s\n' "\$*" >> "${TEST_TMPDIR}/mock-calls-gh"
call_num_file="${TEST_TMPDIR}/gh-call-num"
n=\$(cat "\$call_num_file" 2>/dev/null || echo 0)
n=\$((n + 1))
echo "\$n" > "\$call_num_file"
case "\$n" in
  1) printf '5\n' ;;
  2) printf '{"title":"PR","body":"","files":[],"comments":[]}\n' ;;
  3) printf 'diff --git a/foo.c b/foo.c\n' ;;
  *) exit 0 ;;
esac
ENDMOCK
  chmod +x "$mock_bin"

  local claude_bin="${TEST_TMPDIR}/bin/claude"
  cat > "$claude_bin" <<'ENDCLAUDE'
#!/usr/bin/env bash
printf '%s\n' 'not valid json'
ENDCLAUDE
  chmod +x "$claude_bin"

  run review_run "owner/repo"
  # Skipped PR counts as failed, so exit non-zero
  [ "$status" -ne 0 ]
  [[ "$output" == *"Warning"* ]]
}

# ─── bin/sipag integration ────────────────────────────────────────────────────

@test "bin/sipag review: missing repo arg exits non-zero with usage" {
  run bash "${SIPAG_ROOT}/bin/sipag" review
  [ "$status" -ne 0 ]
  [[ "$output" == *"Usage"* || "$output" == *"missing"* ]]
}

@test "bin/sipag: unknown command exits non-zero" {
  run bash "${SIPAG_ROOT}/bin/sipag" unknowncmd
  [ "$status" -ne 0 ]
  [[ "$output" == *"unknown command"* ]]
}

@test "bin/sipag: no args prints usage and exits 0" {
  run bash "${SIPAG_ROOT}/bin/sipag"
  [ "$status" -eq 0 ]
  [[ "$output" == *"Usage"* ]]
}

@test "bin/sipag --help prints usage and exits 0" {
  run bash "${SIPAG_ROOT}/bin/sipag" --help
  [ "$status" -eq 0 ]
  [[ "$output" == *"review"* ]]
}
