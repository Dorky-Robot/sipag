#!/usr/bin/env bash
# sipag v2 — mock command helpers for BATS

# Creates a mock executable in $TEST_TMPDIR/bin that logs invocations
# and returns configured output/exit code.
#
# Usage:
#   create_mock "claude" 0 "mock output"
#   create_mock "claude" 1 "error"
create_mock() {
  local cmd="$1"
  local exit_code="${2:-0}"
  local mock_output="${3:-}"

  local mock_bin="${TEST_TMPDIR}/bin/${cmd}"
  local mock_log="${TEST_TMPDIR}/mock-${cmd}.log"

  {
    echo '#!/usr/bin/env bash'
    echo "echo \"${cmd} \$*\" >> \"${mock_log}\""
    if [[ -n "$mock_output" ]]; then
      echo "printf '%s\\n' \"${mock_output}\""
    fi
    echo "exit ${exit_code}"
  } >"$mock_bin"
  chmod +x "$mock_bin"
}

# Get all logged calls for a mock command.
get_mock_calls() {
  local cmd="$1"
  local mock_log="${TEST_TMPDIR}/mock-${cmd}.log"
  if [[ -f "$mock_log" ]]; then
    cat "$mock_log"
  fi
}

# Count the number of calls to a mock command.
mock_call_count() {
  local cmd="$1"
  local mock_log="${TEST_TMPDIR}/mock-${cmd}.log"
  if [[ -f "$mock_log" ]]; then
    wc -l <"$mock_log" | tr -d ' '
  else
    echo "0"
  fi
}
