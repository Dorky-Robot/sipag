#!/usr/bin/env bats
# sipag — unit tests for lib/refresh-docs.sh

load ../helpers/test-helpers

setup() {
  setup_common

  export SIPAG_DIR="${TEST_TMPDIR}/sipag"
  mkdir -p "$SIPAG_DIR"

  export WORKER_LOG_DIR="${TEST_TMPDIR}/worker-logs"
  mkdir -p "$WORKER_LOG_DIR"

  source "${SIPAG_ROOT}/lib/refresh-docs.sh"
}

teardown() {
  teardown_common
}

# --- refresh_docs_is_stale ---

@test "refresh_docs_is_stale returns stale when ARCHITECTURE.md has never been committed" {
  # gh api returns empty (no commits for ARCHITECTURE.md)
  cat > "${TEST_TMPDIR}/bin/gh" <<'MOCK'
#!/usr/bin/env bash
# Simulate gh api returning empty for ARCHITECTURE.md commits
if [[ "$*" == *"commits?path=ARCHITECTURE.md"* ]]; then
  echo "null"
  exit 0
fi
exit 1
MOCK
  chmod +x "${TEST_TMPDIR}/bin/gh"

  run refresh_docs_is_stale "owner/repo"
  [[ "$status" -eq 0 ]]  # 0 = stale
}

@test "refresh_docs_is_stale returns stale when a PR was merged after the last doc update" {
  cat > "${TEST_TMPDIR}/bin/gh" <<'MOCK'
#!/usr/bin/env bash
case "$*" in
  *"commits?path=ARCHITECTURE.md"*)
    # ARCHITECTURE.md last updated 2026-01-01
    printf '2026-01-01T10:00:00Z\n'
    ;;
  *"pr list"*"--state merged"*)
    # A PR was merged 2026-01-15 (after the doc update)
    printf '2026-01-15T12:00:00Z\n'
    ;;
  *)
    exit 1
    ;;
esac
exit 0
MOCK
  chmod +x "${TEST_TMPDIR}/bin/gh"

  run refresh_docs_is_stale "owner/repo"
  [[ "$status" -eq 0 ]]  # 0 = stale
}

@test "refresh_docs_is_stale returns up-to-date when doc is newer than last merged PR" {
  cat > "${TEST_TMPDIR}/bin/gh" <<'MOCK'
#!/usr/bin/env bash
case "$*" in
  *"commits?path=ARCHITECTURE.md"*)
    # ARCHITECTURE.md updated 2026-02-20
    printf '2026-02-20T10:00:00Z\n'
    ;;
  *"pr list"*"--state merged"*)
    # Last PR merged 2026-01-15 (before the doc update)
    printf '2026-01-15T12:00:00Z\n'
    ;;
  *)
    exit 1
    ;;
esac
exit 0
MOCK
  chmod +x "${TEST_TMPDIR}/bin/gh"

  run refresh_docs_is_stale "owner/repo"
  [[ "$status" -eq 1 ]]  # 1 = up-to-date
}

@test "refresh_docs_is_stale returns up-to-date when there are no merged PRs" {
  cat > "${TEST_TMPDIR}/bin/gh" <<'MOCK'
#!/usr/bin/env bash
case "$*" in
  *"commits?path=ARCHITECTURE.md"*)
    printf '2026-02-01T10:00:00Z\n'
    ;;
  *"pr list"*"--state merged"*)
    # No merged PRs — empty output
    printf ''
    ;;
  *)
    exit 1
    ;;
esac
exit 0
MOCK
  chmod +x "${TEST_TMPDIR}/bin/gh"

  run refresh_docs_is_stale "owner/repo"
  [[ "$status" -eq 1 ]]  # 1 = up-to-date (no merged PRs → nothing to be stale against)
}

@test "refresh_docs_is_stale returns stale when ARCHITECTURE.md jq returns null (file missing)" {
  cat > "${TEST_TMPDIR}/bin/gh" <<'MOCK'
#!/usr/bin/env bash
case "$*" in
  *"commits?path=ARCHITECTURE.md"*)
    # jq returns null when no commits found
    printf 'null\n'
    ;;
  *)
    exit 1
    ;;
esac
exit 0
MOCK
  chmod +x "${TEST_TMPDIR}/bin/gh"

  run refresh_docs_is_stale "owner/repo"
  [[ "$status" -eq 0 ]]  # 0 = stale (null treated as missing)
}
