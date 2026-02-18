#!/usr/bin/env bash
# sipag — shared test helpers for BATS

# Source this from every .bats file:
#   load ../helpers/test-helpers

SIPAG_TEST_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

setup_common() {
  # Create isolated temp dirs
  export TEST_TMPDIR="${BATS_TEST_TMPDIR}"
  export PROJECT_DIR="${TEST_TMPDIR}/project"
  mkdir -p "$PROJECT_DIR"

  # Set SIPAG_ROOT to the real project root
  export SIPAG_ROOT="$SIPAG_TEST_ROOT"

  # PATH isolation: prepend temp bin so mocks shadow real commands
  export ORIGINAL_PATH="$PATH"
  mkdir -p "${TEST_TMPDIR}/bin"
  export PATH="${TEST_TMPDIR}/bin:${PATH}"

  # Defaults for config vars used by sourced libraries
  export SIPAG_LOG_LEVEL="error"
  export SIPAG_SAFETY_MODE="strict"
  export SIPAG_SOURCE="github"
  export SIPAG_REPO="test-owner/test-repo"
  export SIPAG_BASE_BRANCH="main"
  export SIPAG_CONCURRENCY="2"
  export SIPAG_LABEL_READY="sipag"
  export SIPAG_LABEL_WIP="sipag-wip"
  export SIPAG_LABEL_DONE="sipag-done"
  export SIPAG_TIMEOUT="600"
  export SIPAG_POLL_INTERVAL="60"
  export SIPAG_ALLOWED_TOOLS=""
  export SIPAG_PROMPT_PREFIX=""
}

teardown_common() {
  export PATH="$ORIGINAL_PATH"
}

# --- Assertions ---

assert_json_field() {
  local json="$1" path="$2" expected="$3"
  local actual
  actual=$(echo "$json" | jq -r "$path")
  if [[ "$actual" != "$expected" ]]; then
    echo "assert_json_field failed:"
    echo "  path:     $path"
    echo "  expected: $expected"
    echo "  actual:   $actual"
    return 1
  fi
}

assert_output_contains() {
  local needle="$1"
  if [[ "$output" != *"$needle"* ]]; then
    echo "assert_output_contains failed:"
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

assert_dir_exists() {
  local path="$1"
  if [[ ! -d "$path" ]]; then
    echo "assert_dir_exists failed: $path does not exist"
    return 1
  fi
}

# --- Config helpers ---

create_test_config() {
  local dir="$1"
  shift
  local config_file="${dir}/.sipag"

  # Start with required defaults
  cat >"$config_file" <<'EOF'
SIPAG_SOURCE=github
SIPAG_REPO=test-owner/test-repo
SIPAG_BASE_BRANCH=main
SIPAG_CONCURRENCY=2
SIPAG_LABEL_READY=sipag
SIPAG_LABEL_WIP=sipag-wip
SIPAG_LABEL_DONE=sipag-done
SIPAG_TIMEOUT=600
SIPAG_POLL_INTERVAL=60
SIPAG_SAFETY_MODE=strict
EOF

  # Apply overrides
  for override in "$@"; do
    local key="${override%%=*}"
    local val="${override#*=}"
    if grep -q "^${key}=" "$config_file" 2>/dev/null; then
      local tmp
      tmp=$(mktemp)
      sed "s|^${key}=.*|${key}=${val}|" "$config_file" > "$tmp" && mv "$tmp" "$config_file"
    else
      echo "${key}=${val}" >>"$config_file"
    fi
  done
}
