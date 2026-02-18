#!/usr/bin/env bash
# sipag — mock command helpers for BATS

# Creates a mock executable in $TEST_TMPDIR/bin that logs invocations
# and returns configured output/exit code.
#
# Usage:
#   create_mock "gh" 0 "mock output"
#   create_mock "git" 1 "fatal: error"
create_mock() {
  local cmd="$1"
  local exit_code="${2:-0}"
  local mock_output="${3:-}"

  local mock_bin="${TEST_TMPDIR}/bin/${cmd}"
  local mock_log="${TEST_TMPDIR}/mock-${cmd}.log"

  {
    echo '#!/usr/bin/env bash'
    echo "echo \"\$0 \$*\" >> \"${mock_log}\""
    if [[ -n "$mock_output" ]]; then
      echo "printf '%s\\n' \"${mock_output}\""
    fi
    echo "exit ${exit_code}"
  } >"$mock_bin"
  chmod +x "$mock_bin"
}

# Creates a gh mock that dispatches on subcommand patterns.
# Call set_gh_response to configure responses for specific subcommands.
#
# Usage:
#   create_gh_mock
#   set_gh_response "issue list" 0 "42\n43"
#   set_gh_response "issue edit" 0 ""
create_gh_mock() {
  local mock_bin="${TEST_TMPDIR}/bin/gh"
  local mock_log="${TEST_TMPDIR}/mock-gh.log"
  local mock_dir="${TEST_TMPDIR}/gh-responses"
  mkdir -p "$mock_dir"

  cat >"$mock_bin" <<'GHMOCK'
#!/usr/bin/env bash
MOCK_DIR="__MOCK_DIR__"
MOCK_LOG="__MOCK_LOG__"

echo "$0 $*" >> "$MOCK_LOG"

# Build a key from the first two args (e.g., "issue-list", "pr-create")
KEY="${1:-unknown}-${2:-unknown}"

if [[ -f "${MOCK_DIR}/${KEY}.exit" ]]; then
  EXIT_CODE=$(cat "${MOCK_DIR}/${KEY}.exit")
else
  EXIT_CODE=0
fi

if [[ -f "${MOCK_DIR}/${KEY}.out" ]]; then
  cat "${MOCK_DIR}/${KEY}.out"
fi

exit "$EXIT_CODE"
GHMOCK

  sed -i '' "s|__MOCK_DIR__|${mock_dir}|g" "$mock_bin"
  sed -i '' "s|__MOCK_LOG__|${mock_log}|g" "$mock_bin"
  chmod +x "$mock_bin"
}

# Set the response for a gh subcommand pair.
#
# Usage:
#   set_gh_response "issue list" 0 "42"
#   set_gh_response "issue view" 0 '{"title":"Fix bug","body":"details","number":42,"url":"https://..."}'
set_gh_response() {
  local subcmd="$1"
  local exit_code="$2"
  local output="$3"

  local mock_dir="${TEST_TMPDIR}/gh-responses"
  local key
  key=$(echo "$subcmd" | tr ' ' '-')

  echo "$exit_code" >"${mock_dir}/${key}.exit"
  printf '%s' "$output" >"${mock_dir}/${key}.out"
}

# Set the output for a mock command (replaces the mock with new output).
set_mock_output() {
  local cmd="$1"
  local output="$2"
  local exit_code="${3:-0}"
  create_mock "$cmd" "$exit_code" "$output"
}

# Set the exit code for a mock command.
set_mock_exit() {
  local cmd="$1"
  local exit_code="$2"
  create_mock "$cmd" "$exit_code" ""
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
