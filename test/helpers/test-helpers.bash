#!/usr/bin/env bash
# sipag v2 — shared test helpers for BATS

SIPAG_TEST_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

setup_common() {
  export TEST_TMPDIR="${BATS_TEST_TMPDIR}"

  export SIPAG_ROOT="$SIPAG_TEST_ROOT"

  # PATH isolation: prepend temp bin so mocks shadow real commands
  export ORIGINAL_PATH="$PATH"
  mkdir -p "${TEST_TMPDIR}/bin"
  export PATH="${TEST_TMPDIR}/bin:${PATH}"
}

teardown_common() {
  export PATH="$ORIGINAL_PATH"
}

# --- Assertions ---

assert_output_contains() {
  local needle="$1"
  if [[ "$output" != *"$needle"* ]]; then
    echo "assert_output_contains failed:"
    echo "  needle: $needle"
    echo "  output: $output"
    return 1
  fi
}

assert_output_not_contains() {
  local needle="$1"
  if [[ "$output" == *"$needle"* ]]; then
    echo "assert_output_not_contains failed:"
    echo "  needle: $needle"
    echo "  output: $output"
    return 1
  fi
}

assert_file_exists() {
  local path="$1"
  if [[ ! -f "$path" ]]; then
    echo "assert_file_exists failed: $path does not exist"
    return 1
  fi
}

assert_file_contains() {
  local file="$1"
  local needle="$2"
  if ! grep -qF -- "$needle" "$file"; then
    echo "assert_file_contains failed:"
    echo "  file: $file"
    echo "  needle: $needle"
    echo "  contents: $(cat "$file")"
    return 1
  fi
}
