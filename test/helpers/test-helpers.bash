# sipag — shared test helpers for BATS unit tests

# SIPAG_ROOT: repository root (two levels up from test/helpers/)
SIPAG_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
export SIPAG_ROOT

# setup_common: create an isolated temp directory and prepend a mock bin dir to PATH.
# Sets TEST_TMPDIR and TEST_ORIGINAL_PATH.
setup_common() {
  TEST_TMPDIR="$(mktemp -d)"
  export TEST_TMPDIR
  TEST_ORIGINAL_PATH="$PATH"
  export TEST_ORIGINAL_PATH
  mkdir -p "${TEST_TMPDIR}/bin"
  export PATH="${TEST_TMPDIR}/bin:${PATH}"
}

# teardown_common: restore PATH and remove the temp directory.
teardown_common() {
  if [[ -n "${TEST_ORIGINAL_PATH:-}" ]]; then
    export PATH="$TEST_ORIGINAL_PATH"
  fi
  if [[ -n "${TEST_TMPDIR:-}" && -d "$TEST_TMPDIR" ]]; then
    rm -rf "$TEST_TMPDIR"
  fi
}
