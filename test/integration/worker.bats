#!/usr/bin/env bats
# sipag — worker integration tests (full worker_run with mocks)

load ../helpers/test-helpers
load ../helpers/mock-commands

setup() {
  setup_common
  source "${SIPAG_ROOT}/lib/core/log.sh"
  source "${SIPAG_ROOT}/lib/core/config.sh"
  source "${SIPAG_ROOT}/lib/core/worker.sh"
  source "${SIPAG_ROOT}/lib/sources/github.sh"

  export RUN_DIR="${TEST_TMPDIR}/run"
  mkdir -p "${RUN_DIR}/workers" "${RUN_DIR}/logs"

  # Set up a bare remote repo that can receive pushes
  export BARE_REPO="${TEST_TMPDIR}/bare-repo.git"
  git init --bare "$BARE_REPO" 2>/dev/null

  # Set up a source repo and push an initial commit to the bare remote
  export SOURCE_REPO="${TEST_TMPDIR}/source-repo"
  git init "$SOURCE_REPO" 2>/dev/null
  git -C "$SOURCE_REPO" config user.email "test@test.com"
  git -C "$SOURCE_REPO" config user.name "Test"
  git -C "$SOURCE_REPO" config commit.gpgsign false
  git -C "$SOURCE_REPO" checkout -b main 2>/dev/null
  git -C "$SOURCE_REPO" remote add origin "$BARE_REPO"
  echo "init" > "${SOURCE_REPO}/README.md"
  git -C "$SOURCE_REPO" add README.md
  git -C "$SOURCE_REPO" commit -m "init" 2>/dev/null
  git -C "$SOURCE_REPO" push -u origin main 2>/dev/null

  # Set clone URL to the bare repo (simulates URL-based cloning)
  export SIPAG_CLONE_URL="$BARE_REPO"

  # Mock all external commands
  create_gh_mock
  set_gh_response "issue view" 0 '{"title":"Fix the bug","body":"Please fix it","number":42,"url":"https://github.com/test-owner/test-repo/issues/42"}'
  set_gh_response "issue edit" 0 ""
  set_gh_response "issue comment" 0 ""
  set_gh_response "pr create" 0 "https://github.com/test-owner/test-repo/pull/1"
  set_gh_response "issue close" 0 ""

  # Mock claude to make a commit in the clone dir
  cat > "${TEST_TMPDIR}/bin/claude" <<'CLAUDEMOCK'
#!/usr/bin/env bash
echo "fix applied" > fix.txt
git add fix.txt
git -c user.email="test@test.com" -c user.name="Test" commit -m "Fix the bug" 2>/dev/null
CLAUDEMOCK
  chmod +x "${TEST_TMPDIR}/bin/claude"

  # Mock timeout to just run the command directly
  cat > "${TEST_TMPDIR}/bin/timeout" <<'TIMEOUTMOCK'
#!/usr/bin/env bash
shift  # skip the timeout value
exec "$@"
TIMEOUTMOCK
  chmod +x "${TEST_TMPDIR}/bin/timeout"

  export SIPAG_SAFETY_MODE="yolo"
  export SIPAG_BASE_BRANCH="main"

  # Override gpgsign for all git operations in tests (scoped to test process)
  export GIT_CONFIG_COUNT=1
  export GIT_CONFIG_KEY_0="commit.gpgsign"
  export GIT_CONFIG_VALUE_0="false"
}

teardown() {
  teardown_common
}

@test "worker_run: happy path through full lifecycle (URL clone)" {
  run worker_run "42" "$RUN_DIR"
  [[ "$status" -eq 0 ]]

  # State file should show done
  local json
  json=$(cat "${RUN_DIR}/workers/42.json")
  assert_json_field "$json" ".status" "done"
  assert_json_field "$json" ".pr_url" "https://github.com/test-owner/test-repo/pull/1"
}

@test "worker_run: failure at task fetch" {
  set_gh_response "issue view" 1 ""

  run worker_run "42" "$RUN_DIR"
  [[ "$status" -ne 0 ]]
}

@test "worker_run: failure at claim" {
  set_gh_response "issue edit" 1 ""

  run worker_run "42" "$RUN_DIR"
  [[ "$status" -ne 0 ]]

  local json
  json=$(cat "${RUN_DIR}/workers/42.json")
  assert_json_field "$json" ".status" "failed"
}

@test "worker_run: no commits produced" {
  cat > "${TEST_TMPDIR}/bin/claude" <<'CLAUDEMOCK'
#!/usr/bin/env bash
echo "I analyzed the issue but made no changes"
CLAUDEMOCK
  chmod +x "${TEST_TMPDIR}/bin/claude"

  run worker_run "42" "$RUN_DIR"
  [[ "$status" -ne 0 ]]

  local json
  json=$(cat "${RUN_DIR}/workers/42.json")
  assert_json_field "$json" ".status" "failed"
  local err
  err=$(echo "$json" | jq -r '.error')
  [[ "$err" == *"commits"* ]]
}

@test "worker_run: claude exits non-zero" {
  cat > "${TEST_TMPDIR}/bin/claude" <<'CLAUDEMOCK'
#!/usr/bin/env bash
echo "error: something went wrong"
exit 1
CLAUDEMOCK
  chmod +x "${TEST_TMPDIR}/bin/claude"

  run worker_run "42" "$RUN_DIR"
  [[ "$status" -ne 0 ]]

  local json
  json=$(cat "${RUN_DIR}/workers/42.json")
  assert_json_field "$json" ".status" "failed"
}

@test "worker_run: push failure" {
  # Use a non-existent URL as clone URL (but let clone succeed from bare repo first)
  # Then change the remote to something invalid after clone
  cat > "${TEST_TMPDIR}/bin/claude" <<'CLAUDEMOCK'
#!/usr/bin/env bash
git remote set-url origin "https://invalid.example.com/no-such-repo.git"
echo "fix applied" > fix.txt
git add fix.txt
git -c user.email="test@test.com" -c user.name="Test" commit -m "Fix the bug" 2>/dev/null
CLAUDEMOCK
  chmod +x "${TEST_TMPDIR}/bin/claude"

  run worker_run "42" "$RUN_DIR"
  [[ "$status" -ne 0 ]]

  local json
  json=$(cat "${RUN_DIR}/workers/42.json")
  assert_json_field "$json" ".status" "failed"
  local err
  err=$(echo "$json" | jq -r '.error')
  [[ "$err" == *"push"* ]]
}

@test "worker_run: PR creation failure" {
  set_gh_response "pr create" 1 ""

  run worker_run "42" "$RUN_DIR"
  [[ "$status" -ne 0 ]]

  local json
  json=$(cat "${RUN_DIR}/workers/42.json")
  assert_json_field "$json" ".status" "failed"
}

@test "worker_run: branch naming convention" {
  run worker_run "42" "$RUN_DIR"
  [[ "$status" -eq 0 ]]

  local json
  json=$(cat "${RUN_DIR}/workers/42.json")
  local branch
  branch=$(echo "$json" | jq -r '.branch')
  [[ "$branch" == sipag/42-* ]]
}

@test "worker_run: safety hook setup when mode != yolo" {
  export SIPAG_SAFETY_MODE="strict"

  # Mock claude to check for hooks then fail
  cat > "${TEST_TMPDIR}/bin/claude" <<'CLAUDEMOCK'
#!/usr/bin/env bash
if [[ -f .claude/settings.local.json ]]; then
  echo "HOOKS_PRESENT"
fi
exit 1
CLAUDEMOCK
  chmod +x "${TEST_TMPDIR}/bin/claude"

  run worker_run "42" "$RUN_DIR"

  local log_file="${RUN_DIR}/logs/worker-42.log"
  [[ -f "$log_file" ]]
  grep -q "HOOKS_PRESENT" "$log_file"
}

@test "worker_run: clones from SIPAG_CLONE_URL, not local dir" {
  # worker_run no longer takes a project_dir arg
  run worker_run "42" "$RUN_DIR"
  [[ "$status" -eq 0 ]]

  # The clone should have come from BARE_REPO via SIPAG_CLONE_URL
  local json
  json=$(cat "${RUN_DIR}/workers/42.json")
  assert_json_field "$json" ".status" "done"
}
