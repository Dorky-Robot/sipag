# sipag — mock command infrastructure for BATS unit tests
#
# Mock executables are created in $TEST_TMPDIR/bin, which setup_common()
# prepends to PATH so they shadow real commands during tests.

# create_mock <command> <exit_code> <output>
#
# Creates a mock executable that:
#   - prints <output> to stdout on each invocation
#   - appends all arguments (one line per invocation) to a call-log file
#   - exits with <exit_code>
#
# The call-log is stored at $TEST_TMPDIR/mock-calls-<command>.
create_mock() {
  local cmd="$1"
  local exit_code="$2"
  local output="$3"
  # Sanitise command name for use as a filename (replace non-alphanum with _)
  local safe_name
  safe_name="${cmd//[^a-zA-Z0-9]/_}"
  local mock_bin="${TEST_TMPDIR}/bin/${cmd}"
  local call_log="${TEST_TMPDIR}/mock-calls-${safe_name}"

  cat > "$mock_bin" <<MOCKSCRIPT
#!/usr/bin/env bash
printf '%s\n' "\$*" >> "${call_log}"
printf '%s\n' "${output}"
exit ${exit_code}
MOCKSCRIPT
  chmod +x "$mock_bin"
}

# mock_call_count <command>
#
# Prints the number of times create_mock'd <command> has been invoked.
mock_call_count() {
  local cmd="$1"
  local safe_name
  safe_name="${cmd//[^a-zA-Z0-9]/_}"
  local call_log="${TEST_TMPDIR}/mock-calls-${safe_name}"
  if [[ -f "$call_log" ]]; then
    wc -l < "$call_log" | tr -d ' '
  else
    echo "0"
  fi
}

# get_mock_calls <command>
#
# Prints all recorded argument strings for invocations of <command>,
# one line per call.
get_mock_calls() {
  local cmd="$1"
  local safe_name
  safe_name="${cmd//[^a-zA-Z0-9]/_}"
  local call_log="${TEST_TMPDIR}/mock-calls-${safe_name}"
  if [[ -f "$call_log" ]]; then
    cat "$call_log"
  else
    echo ""
  fi
}
