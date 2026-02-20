#!/usr/bin/env bats
# sipag v2 — unit tests for lib/repo.sh

load ../helpers/test-helpers

setup() {
  setup_common
  source "${SIPAG_ROOT}/lib/repo.sh"
  export SIPAG_DIR="${TEST_TMPDIR}/sipag"
  mkdir -p "${SIPAG_DIR}"
}

teardown() {
  teardown_common
}

# --- repo_url ---

@test "repo_url: finds url for registered name" {
  echo "myrepo=https://github.com/org/myrepo" >"${SIPAG_DIR}/repos.conf"

  run repo_url "myrepo"
  [[ "$status" -eq 0 ]]
  assert_output_contains "https://github.com/org/myrepo"
}

@test "repo_url: returns 1 for unknown name" {
  echo "other=https://github.com/org/other" >"${SIPAG_DIR}/repos.conf"

  run repo_url "nonexistent"
  [[ "$status" -eq 1 ]]
}

@test "repo_url: returns 1 when repos.conf missing" {
  run repo_url "anyname"
  [[ "$status" -eq 1 ]]
}

@test "repo_url: finds correct entry among multiple repos" {
  cat >"${SIPAG_DIR}/repos.conf" <<'EOF'
alpha=https://github.com/org/alpha
beta=https://github.com/org/beta
gamma=https://github.com/org/gamma
EOF

  run repo_url "beta"
  [[ "$status" -eq 0 ]]
  assert_output_contains "https://github.com/org/beta"
}

@test "repo_url: does not match partial name" {
  echo "foo=https://github.com/org/foo" >"${SIPAG_DIR}/repos.conf"

  run repo_url "fo"
  [[ "$status" -eq 1 ]]
}

@test "repo_url: handles url containing equals sign" {
  echo "myrepo=https://example.com/path?foo=bar" >"${SIPAG_DIR}/repos.conf"

  run repo_url "myrepo"
  [[ "$status" -eq 0 ]]
  assert_output_contains "https://example.com/path?foo=bar"
}
