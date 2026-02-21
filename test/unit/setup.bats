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

  # Mock docker: all subcommands succeed (info=running, image inspect=exists)
  cat >"${TEST_TMPDIR}/bin/docker" <<'MOCK'
#!/usr/bin/env bash
exit 0
MOCK
  chmod +x "${TEST_TMPDIR}/bin/docker"
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

@test "setup_run: fails when docker is missing" {
  rm -f "${TEST_TMPDIR}/bin/docker"
  PATH="${TEST_TMPDIR}/bin" run setup_run
  [[ "$status" -ne 0 ]]
  assert_output_contains "Docker not installed"
}

@test "setup_run: fails when docker is not running" {
  # docker info returns non-zero (daemon not running)
  cat >"${TEST_TMPDIR}/bin/docker" <<'MOCK'
#!/usr/bin/env bash
if [[ "$1" == "info" ]]; then
  exit 1
fi
exit 0
MOCK
  chmod +x "${TEST_TMPDIR}/bin/docker"

  run setup_run
  [[ "$status" -ne 0 ]]
  assert_output_contains "Docker not running"
}

# --- ~/.sipag/ directory creation ---

@test "setup_run: creates ~/.sipag/ subdirectories" {
  # gh auth status succeeds only for 'auth status'
  cat >"${TEST_TMPDIR}/bin/gh" <<'MOCK'
#!/usr/bin/env bash
if [[ "$1 $2" == "auth status" ]]; then
  exit 0
fi
exit 0
MOCK
  chmod +x "${TEST_TMPDIR}/bin/gh"

  # Pre-create token so auth check passes cleanly
  mkdir -p "$HOME/.sipag"
  echo "test-token" >"$HOME/.sipag/token"

  run setup_run
  [[ "$status" -eq 0 ]]
  [[ -d "$HOME/.sipag/queue" ]]
  [[ -d "$HOME/.sipag/running" ]]
  [[ -d "$HOME/.sipag/done" ]]
  [[ -d "$HOME/.sipag/failed" ]]
  [[ -d "$HOME/.sipag/hooks" ]]
}

# --- _setup_dirs ---

@test "_setup_dirs: creates all subdirectories including hooks" {
  run _setup_dirs
  [[ "$status" -eq 0 ]]
  [[ -d "$HOME/.sipag/queue" ]]
  [[ -d "$HOME/.sipag/running" ]]
  [[ -d "$HOME/.sipag/done" ]]
  [[ -d "$HOME/.sipag/failed" ]]
  [[ -d "$HOME/.sipag/hooks" ]]
}

@test "_setup_dirs: is idempotent" {
  _setup_dirs
  run _setup_dirs
  [[ "$status" -eq 0 ]]
  assert_output_contains "already exist"
}

# --- _setup_auth ---

@test "_setup_auth: reports OK when token already exists" {
  mkdir -p "$HOME/.sipag"
  echo "existing-token" >"$HOME/.sipag/token"

  run _setup_auth
  [[ "$status" -eq 0 ]]
  assert_output_contains "OAuth token configured"
}

@test "_setup_auth: copies token when claude setup-token creates it" {
  # claude mock creates ~/.claude/token when called with setup-token
  cat >"${TEST_TMPDIR}/bin/claude" <<'MOCK'
#!/usr/bin/env bash
if [[ "$1" == "setup-token" ]]; then
  mkdir -p "$HOME/.claude"
  echo "fresh-oauth-token" >"$HOME/.claude/token"
  exit 0
fi
exit 0
MOCK
  chmod +x "${TEST_TMPDIR}/bin/claude"

  run _setup_auth
  [[ "$status" -eq 0 ]]
  assert_output_contains "OAuth token configured"
  [[ -f "$HOME/.sipag/token" ]]
  grep -q "fresh-oauth-token" "$HOME/.sipag/token"
}

@test "_setup_auth: reports error when claude setup-token produces no token" {
  # claude mock succeeds but does not create ~/.claude/token
  create_mock "claude" 0

  run _setup_auth
  [[ "$status" -eq 0 ]]  # auth is non-fatal — setup continues
  assert_output_contains "OAuth token missing"
}

@test "_setup_auth: reports ANTHROPIC_API_KEY when set" {
  mkdir -p "$HOME/.sipag"
  echo "test-token" >"$HOME/.sipag/token"

  ANTHROPIC_API_KEY="sk-test" run _setup_auth
  assert_output_contains "ANTHROPIC_API_KEY set"
}

@test "_setup_auth: mentions ANTHROPIC_API_KEY as optional when not set" {
  mkdir -p "$HOME/.sipag"
  echo "test-token" >"$HOME/.sipag/token"

  unset ANTHROPIC_API_KEY
  run _setup_auth
  assert_output_contains "ANTHROPIC_API_KEY not set"
  assert_output_contains "optional"
}

# --- _setup_docker_image ---

@test "_setup_docker_image: reports OK when image already exists" {
  # Default docker mock returns 0 for all subcommands (image inspect = exists)
  run _setup_docker_image
  [[ "$status" -eq 0 ]]
  assert_output_contains "exists"
}

@test "_setup_docker_image: fails when image missing and build fails" {
  # docker: image inspect fails, build also fails
  cat >"${TEST_TMPDIR}/bin/docker" <<'MOCK'
#!/usr/bin/env bash
if [[ "$1" == "image" ]]; then
  exit 1  # image not found
fi
if [[ "$1" == "build" ]]; then
  exit 1  # build failed
fi
exit 0
MOCK
  chmod +x "${TEST_TMPDIR}/bin/docker"

  run _setup_docker_image
  [[ "$status" -ne 0 ]]
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
