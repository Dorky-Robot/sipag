#!/usr/bin/env bats
# sipag — unit tests for lib/setup.sh

load ../helpers/test-helpers
load ../helpers/mock-commands

setup() {
  setup_common
  source "${SIPAG_ROOT}/lib/setup.sh"

  # Redirect HOME so tests never touch the real user's settings
  export HOME="${TEST_TMPDIR}/home"
  mkdir -p "$HOME"

  # Mock gh and claude as present and working by default
  create_mock "gh" 0
  create_mock "claude" 0
}

teardown() {
  teardown_common
}

# --- Prerequisite checks ---

@test "setup_run: fails when gh is missing" {
  # Isolate PATH to only the test bin dir (no system commands) so gh is truly absent
  rm -f "${TEST_TMPDIR}/bin/gh"
  PATH="${TEST_TMPDIR}/bin" run setup_run
  [[ "$status" -ne 0 ]]
  assert_output_contains "gh CLI required"
}

@test "setup_run: fails when claude is missing" {
  # Isolate PATH so claude is truly absent
  rm -f "${TEST_TMPDIR}/bin/claude"
  PATH="${TEST_TMPDIR}/bin" run setup_run
  [[ "$status" -ne 0 ]]
  assert_output_contains "claude CLI required"
}

@test "setup_run: fails when gh not authenticated" {
  # gh auth status returns non-zero
  create_mock "gh" 1

  run setup_run
  [[ "$status" -ne 0 ]]
  assert_output_contains "gh not authenticated"
}

# --- ~/.sipag/ directory creation ---

@test "setup_run: creates ~/.sipag/ directory" {
  # gh auth status succeeds only for 'auth status'
  cat >"${TEST_TMPDIR}/bin/gh" <<'MOCK'
#!/usr/bin/env bash
if [[ "$1 $2" == "auth status" ]]; then
  exit 0
fi
exit 0
MOCK
  chmod +x "${TEST_TMPDIR}/bin/gh"

  run setup_run
  [[ "$status" -eq 0 ]]
  [[ -d "$HOME/.sipag" ]]
}

# --- ~/.claude/settings.json creation and merging ---

@test "_setup_merge_with_jq: creates settings.json when absent" {
  command -v jq >/dev/null 2>&1 || skip "jq not available"

  local settings="$HOME/.claude/settings.json"
  mkdir -p "$HOME/.claude"

  _setup_merge_with_jq "$settings"

  [[ -f "$settings" ]]
  grep -q "Bash(gh issue \*)" "$settings"
  grep -q "Bash(gh pr \*)" "$settings"
  grep -q "Bash(gh label \*)" "$settings"
}

@test "_setup_merge_with_jq: merges into existing settings without overwriting" {
  command -v jq >/dev/null 2>&1 || skip "jq not available"

  local settings="$HOME/.claude/settings.json"
  mkdir -p "$HOME/.claude"
  cat >"$settings" <<'JSON'
{
  "someOtherKey": "preserved",
  "permissions": {
    "allow": ["Bash(git *)"]
  }
}
JSON

  _setup_merge_with_jq "$settings"

  # Existing key preserved
  grep -q '"someOtherKey"' "$settings"
  # Existing permission preserved
  grep -q 'Bash(git \*)' "$settings"
  # New permissions added
  grep -q 'Bash(gh issue \*)' "$settings"
}

@test "_setup_merge_with_jq: is idempotent (no duplicates)" {
  command -v jq >/dev/null 2>&1 || skip "jq not available"

  local settings="$HOME/.claude/settings.json"
  mkdir -p "$HOME/.claude"

  _setup_merge_with_jq "$settings"
  _setup_merge_with_jq "$settings"

  # Count occurrences of one permission — should be exactly 1
  local count
  count=$(grep -c 'Bash(gh issue \*)' "$settings")
  [[ "$count" -eq 1 ]]
}

@test "_setup_merge_with_python: creates settings.json when absent" {
  command -v python3 >/dev/null 2>&1 || skip "python3 not available"

  local settings="$HOME/.claude/settings.json"
  mkdir -p "$HOME/.claude"

  _setup_merge_with_python "$settings"

  [[ -f "$settings" ]]
  grep -q "Bash(gh issue \*)" "$settings"
  grep -q "Bash(gh pr \*)" "$settings"
  grep -q "Bash(gh label \*)" "$settings"
}

@test "_setup_merge_with_python: merges into existing settings without overwriting" {
  command -v python3 >/dev/null 2>&1 || skip "python3 not available"

  local settings="$HOME/.claude/settings.json"
  mkdir -p "$HOME/.claude"
  cat >"$settings" <<'JSON'
{
  "someOtherKey": "preserved",
  "permissions": {
    "allow": ["Bash(git *)"]
  }
}
JSON

  _setup_merge_with_python "$settings"

  grep -q '"someOtherKey"' "$settings"
  grep -q 'Bash(git \*)' "$settings"
  grep -q 'Bash(gh issue \*)' "$settings"
}

@test "_setup_merge_with_python: is idempotent (no duplicates)" {
  command -v python3 >/dev/null 2>&1 || skip "python3 not available"

  local settings="$HOME/.claude/settings.json"
  mkdir -p "$HOME/.claude"

  _setup_merge_with_python "$settings"
  _setup_merge_with_python "$settings"

  local count
  count=$(grep -c 'Bash(gh issue \*)' "$settings")
  [[ "$count" -eq 1 ]]
}

@test "_setup_claude_permissions: skips when already configured" {
  local settings="$HOME/.claude/settings.json"
  mkdir -p "$HOME/.claude"
  cat >"$settings" <<'JSON'
{
  "permissions": {
    "allow": [
      "Bash(gh issue *)",
      "Bash(gh pr *)",
      "Bash(gh label *)"
    ]
  }
}
JSON

  run _setup_claude_permissions
  [[ "$status" -eq 0 ]]
  assert_output_contains "already configured"
}
