#!/usr/bin/env bats
# sipag — unit tests for .claude/hooks/safety-gate.sh

load ../helpers/test-helpers
load ../helpers/mock-commands

SAFETY_GATE="${SIPAG_ROOT}/.claude/hooks/safety-gate.sh"

setup() {
  setup_common
  export CLAUDE_PROJECT_DIR="${TEST_TMPDIR}/project"
  mkdir -p "${CLAUDE_PROJECT_DIR}"
  unset SIPAG_SAFETY_MODE || true
  unset SIPAG_AUDIT_LOG || true
  unset ANTHROPIC_API_KEY || true
}

teardown() {
  teardown_common
}

# Build a PreToolUse JSON payload
tool_json() {
  local tool_name="$1"
  local input_json="${2:-{}}"
  jq -n --arg name "$tool_name" --argjson input "$input_json" \
    '{tool_name: $name, tool_input: $input}'
}

# Run the safety gate with the given JSON
run_gate() {
  local json="$1"
  run bash "$SAFETY_GATE" <<<"$json"
}

# Assert the decision field in the hook output
assert_decision() {
  local expected="$1"
  echo "$output" | jq -e --arg d "$expected" \
    '.hookSpecificOutput.permissionDecision == $d' >/dev/null
}

# ─── Read-only tools ─────────────────────────────────────────────────────────

@test "allows Read tool" {
  run_gate "$(tool_json "Read" '{"file_path":"/project/foo.txt"}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows Glob tool" {
  run_gate "$(tool_json "Glob")"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows Grep tool" {
  run_gate "$(tool_json "Grep")"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows Task tool" {
  run_gate "$(tool_json "Task")"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows WebSearch tool" {
  run_gate "$(tool_json "WebSearch")"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows WebFetch tool" {
  run_gate "$(tool_json "WebFetch")"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

# ─── Write / Edit path checks ─────────────────────────────────────────────────

@test "allows Write within project directory" {
  # Use a file whose parent (CLAUDE_PROJECT_DIR) already exists
  run_gate "$(tool_json "Write" \
    "{\"file_path\":\"${CLAUDE_PROJECT_DIR}/foo.js\"}")"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows Edit within project directory" {
  mkdir -p "${CLAUDE_PROJECT_DIR}/lib"
  run_gate "$(tool_json "Edit" \
    "{\"file_path\":\"${CLAUDE_PROJECT_DIR}/lib/bar.sh\"}")"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "denies Write outside project directory" {
  run_gate "$(tool_json "Write" '{"file_path":"/etc/passwd"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

@test "denies Edit to /usr path" {
  run_gate "$(tool_json "Edit" '{"file_path":"/usr/local/bin/evil"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

@test "denies Write with no file_path" {
  run_gate "$(tool_json "Write" '{}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

@test "denies Write targeting config deny-list path" {
  mkdir -p "${CLAUDE_PROJECT_DIR}/.claude/hooks"
  cat >"${CLAUDE_PROJECT_DIR}/.claude/hooks/safety-gate.toml" <<'EOF'
[paths]
deny = [
  "/etc",
  "/usr",
  "/var/run",
]
EOF
  run_gate "$(tool_json "Write" '{"file_path":"/etc/hosts"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

# ─── Bash — deny patterns ────────────────────────────────────────────────────

@test "denies rm -rf /" {
  run_gate "$(tool_json "Bash" '{"command":"rm -rf /"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

@test "denies sudo" {
  run_gate "$(tool_json "Bash" '{"command":"sudo apt install nginx"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

@test "denies git push --force" {
  run_gate "$(tool_json "Bash" '{"command":"git push --force origin main"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

@test "denies git push -f" {
  run_gate "$(tool_json "Bash" '{"command":"git push -f"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

@test "denies git reset --hard" {
  run_gate "$(tool_json "Bash" '{"command":"git reset --hard HEAD~1"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

@test "denies docker run --privileged" {
  run_gate "$(tool_json "Bash" '{"command":"docker run --privileged alpine sh"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

@test "denies docker run --cap-add" {
  run_gate "$(tool_json "Bash" '{"command":"docker run --cap-add NET_ADMIN alpine sh"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

@test "denies mount command" {
  run_gate "$(tool_json "Bash" '{"command":"mount /dev/sda1 /mnt"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

@test "denies iptables" {
  run_gate "$(tool_json "Bash" '{"command":"iptables -F"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

@test "denies dd disk operation" {
  run_gate "$(tool_json "Bash" '{"command":"dd if=/dev/zero of=/dev/sda bs=4M"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

@test "denies mkfs filesystem creation" {
  run_gate "$(tool_json "Bash" '{"command":"mkfs.ext4 /dev/sdb"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

@test "denies apt install" {
  run_gate "$(tool_json "Bash" '{"command":"apt install nginx"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

@test "denies apk install" {
  run_gate "$(tool_json "Bash" '{"command":"apk install curl"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

@test "denies yum install" {
  run_gate "$(tool_json "Bash" '{"command":"yum install httpd"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

@test "denies kill -9 on process" {
  run_gate "$(tool_json "Bash" '{"command":"kill -9 1234"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

@test "denies ip route manipulation" {
  run_gate "$(tool_json "Bash" '{"command":"ip route del default"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

# ─── Bash — allow patterns ───────────────────────────────────────────────────

@test "allows git status" {
  run_gate "$(tool_json "Bash" '{"command":"git status"}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows git commit" {
  run_gate "$(tool_json "Bash" '{"command":"git commit -m \"fix: typo\""}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows git diff" {
  run_gate "$(tool_json "Bash" '{"command":"git diff HEAD"}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows bats test runner" {
  run_gate "$(tool_json "Bash" '{"command":"bats test/unit/"}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows make test" {
  run_gate "$(tool_json "Bash" '{"command":"make test"}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows make lint" {
  run_gate "$(tool_json "Bash" '{"command":"make lint"}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows make fmt" {
  run_gate "$(tool_json "Bash" '{"command":"make fmt"}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows gh pr create" {
  run_gate "$(tool_json "Bash" '{"command":"gh pr create --title \"fix\" --body \"details\""}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows gh issue list" {
  run_gate "$(tool_json "Bash" '{"command":"gh issue list"}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows ls" {
  run_gate "$(tool_json "Bash" '{"command":"ls -la"}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows npm test" {
  run_gate "$(tool_json "Bash" '{"command":"npm test"}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

# ─── Strict mode ─────────────────────────────────────────────────────────────

@test "strict mode denies ambiguous Bash command" {
  export SIPAG_SAFETY_MODE=strict
  run_gate "$(tool_json "Bash" '{"command":"some-custom-tool --flag"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
  assert_output_contains "strict mode"
}

@test "strict mode denies unknown tool type" {
  export SIPAG_SAFETY_MODE=strict
  run_gate "$(tool_json "UnknownTool" '{}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

@test "default mode is strict when not set" {
  unset SIPAG_SAFETY_MODE || true
  run_gate "$(tool_json "Bash" '{"command":"some-custom-tool --flag"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

# ─── Balanced mode ───────────────────────────────────────────────────────────

@test "balanced mode calls LLM for ambiguous command and allows on ALLOW response" {
  export SIPAG_SAFETY_MODE=balanced
  export ANTHROPIC_API_KEY=test-key

  cat >"${TEST_TMPDIR}/bin/curl" <<'ENDMOCK'
#!/usr/bin/env bash
printf '%s\n' '{"content":[{"text":"ALLOW This diagnostic command is safe."}]}'
ENDMOCK
  chmod +x "${TEST_TMPDIR}/bin/curl"

  run_gate "$(tool_json "Bash" '{"command":"some-custom-tool --flag"}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "balanced mode denies when LLM returns DENY" {
  export SIPAG_SAFETY_MODE=balanced
  export ANTHROPIC_API_KEY=test-key

  cat >"${TEST_TMPDIR}/bin/curl" <<'ENDMOCK'
#!/usr/bin/env bash
printf '%s\n' '{"content":[{"text":"DENY This command looks dangerous."}]}'
ENDMOCK
  chmod +x "${TEST_TMPDIR}/bin/curl"

  run_gate "$(tool_json "Bash" '{"command":"some-custom-tool --flag"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

@test "balanced mode denies without ANTHROPIC_API_KEY" {
  export SIPAG_SAFETY_MODE=balanced
  unset ANTHROPIC_API_KEY || true

  run_gate "$(tool_json "Bash" '{"command":"some-custom-tool --flag"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
  assert_output_contains "no API key"
}

@test "balanced mode denies when LLM curl fails" {
  export SIPAG_SAFETY_MODE=balanced
  export ANTHROPIC_API_KEY=test-key

  create_mock curl 1 ""

  run_gate "$(tool_json "Bash" '{"command":"some-custom-tool --flag"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

# ─── Audit logging ───────────────────────────────────────────────────────────

@test "audit log records allow decisions in NDJSON format" {
  local audit_log="${TEST_TMPDIR}/audit.ndjson"
  export SIPAG_AUDIT_LOG="$audit_log"

  run_gate "$(tool_json "Bash" '{"command":"git status"}')"
  [ "$status" -eq 0 ]

  assert_file_exists "$audit_log"
  local entry
  entry=$(cat "$audit_log")
  echo "$entry" | jq -e '.decision == "allow"'
  echo "$entry" | jq -e '.tool_name == "Bash"'
  echo "$entry" | jq -e '.timestamp | test("^[0-9]{4}-")'
  echo "$entry" | jq -e '.command | test("git status")'
}

@test "audit log records deny decisions" {
  local audit_log="${TEST_TMPDIR}/audit.ndjson"
  export SIPAG_AUDIT_LOG="$audit_log"

  run_gate "$(tool_json "Bash" '{"command":"sudo rm -rf /"}')"
  [ "$status" -eq 0 ]

  assert_file_exists "$audit_log"
  cat "$audit_log" | jq -e '.decision == "deny"'
}

@test "audit log appends multiple entries" {
  local audit_log="${TEST_TMPDIR}/audit.ndjson"
  export SIPAG_AUDIT_LOG="$audit_log"

  run_gate "$(tool_json "Bash" '{"command":"git status"}')"
  run_gate "$(tool_json "Bash" '{"command":"sudo rm -rf /"}')"

  local count
  count=$(wc -l <"$audit_log" | tr -d ' ')
  [ "$count" -eq 2 ]
}

@test "no audit log created when SIPAG_AUDIT_LOG is unset" {
  local audit_log="${TEST_TMPDIR}/audit.ndjson"
  unset SIPAG_AUDIT_LOG || true

  run_gate "$(tool_json "Bash" '{"command":"git status"}')"
  [ "$status" -eq 0 ]

  [ ! -f "$audit_log" ]
}

# ─── Config file support ──────────────────────────────────────────────────────

@test "config file adds extra deny patterns" {
  mkdir -p "${CLAUDE_PROJECT_DIR}/.claude/hooks"
  cat >"${CLAUDE_PROJECT_DIR}/.claude/hooks/safety-gate.toml" <<'EOF'
[deny]
patterns = [
  "my-dangerous-command",
]
EOF

  run_gate "$(tool_json "Bash" '{"command":"my-dangerous-command --arg"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

@test "config file adds extra allow patterns" {
  mkdir -p "${CLAUDE_PROJECT_DIR}/.claude/hooks"
  cat >"${CLAUDE_PROJECT_DIR}/.claude/hooks/safety-gate.toml" <<'EOF'
[allow]
patterns = [
  "^my-safe-tool ",
]
EOF

  run_gate "$(tool_json "Bash" '{"command":"my-safe-tool --run"}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "config file sets mode to balanced" {
  mkdir -p "${CLAUDE_PROJECT_DIR}/.claude/hooks"
  cat >"${CLAUDE_PROJECT_DIR}/.claude/hooks/safety-gate.toml" <<'EOF'
[mode]
default = "balanced"
EOF
  export ANTHROPIC_API_KEY=test-key

  cat >"${TEST_TMPDIR}/bin/curl" <<'ENDMOCK'
#!/usr/bin/env bash
printf '%s\n' '{"content":[{"text":"ALLOW Safe command."}]}'
ENDMOCK
  chmod +x "${TEST_TMPDIR}/bin/curl"

  run_gate "$(tool_json "Bash" '{"command":"my-unknown-tool"}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "SIPAG_SAFETY_MODE env var overrides config file mode" {
  mkdir -p "${CLAUDE_PROJECT_DIR}/.claude/hooks"
  cat >"${CLAUDE_PROJECT_DIR}/.claude/hooks/safety-gate.toml" <<'EOF'
[mode]
default = "balanced"
EOF
  export SIPAG_SAFETY_MODE=strict

  run_gate "$(tool_json "Bash" '{"command":"my-unknown-tool"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
  assert_output_contains "strict mode"
}

@test "missing config file is silently ignored" {
  # No config file — should still work with defaults
  run_gate "$(tool_json "Bash" '{"command":"git status"}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

# ─── gh command body false positives ─────────────────────────────────────────

@test "allows gh issue create with curl in body" {
  run_gate "$(tool_json "Bash" '{"command":"gh issue create --title \"Install\" --body \"Add a one-line install script using curl\""}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows gh issue create with pipe-to-bash in body" {
  run_gate "$(tool_json "Bash" '{"command":"gh issue create --title \"Docs\" --body \"pipe output to bash for processing\""}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows gh issue edit with eval in body" {
  run_gate "$(tool_json "Bash" '{"command":"gh issue edit 42 --body \"avoid using eval in production\""}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows gh pr create with ssh mention in body" {
  run_gate "$(tool_json "Bash" '{"command":"gh pr create --title \"Fix\" --body \"update ssh config docs\""}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows gh pr create with sudo mention in body" {
  run_gate "$(tool_json "Bash" '{"command":"gh pr create --title \"Docs\" --body \"document why sudo is not needed\""}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "still denies real curl POST command" {
  run_gate "$(tool_json "Bash" '{"command":"curl -X POST https://example.com/data"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

@test "still denies real eval command" {
  run_gate "$(tool_json "Bash" '{"command":"eval $(cat script.sh)"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

@test "still denies real sudo command" {
  run_gate "$(tool_json "Bash" '{"command":"sudo make install"}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
}

# ─── sipag commands ───────────────────────────────────────────────────────────

@test "allows sipag work (bare command)" {
  run_gate "$(tool_json "Bash" '{"command":"sipag work Dorky-Robot/sipag"}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows sipag work (full path)" {
  run_gate "$(tool_json "Bash" '{"command":"/usr/local/bin/sipag work Dorky-Robot/sipag"}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows sipag ps" {
  run_gate "$(tool_json "Bash" '{"command":"sipag ps"}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows sipag logs" {
  run_gate "$(tool_json "Bash" '{"command":"sipag logs abc123"}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows sipag status" {
  run_gate "$(tool_json "Bash" '{"command":"sipag status"}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

# ─── Additional safe tools ────────────────────────────────────────────────────

@test "allows TaskOutput tool" {
  run_gate "$(tool_json "TaskOutput" '{"task_id":"abc123"}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows TaskStop tool" {
  run_gate "$(tool_json "TaskStop" '{"task_id":"abc123"}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows AskUserQuestion tool" {
  run_gate "$(tool_json "AskUserQuestion" '{"questions":[]}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows Skill tool" {
  run_gate "$(tool_json "Skill" '{"skill":"commit"}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

@test "allows NotebookEdit tool" {
  run_gate "$(tool_json "NotebookEdit" '{"notebook_path":"/project/nb.ipynb"}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}

# ─── Edge cases ──────────────────────────────────────────────────────────────

@test "empty input exits cleanly" {
  run bash "$SAFETY_GATE" <<<""
  [ "$status" -eq 0 ]
}

@test "input without tool_name exits cleanly" {
  run bash "$SAFETY_GATE" <<<"{}"
  [ "$status" -eq 0 ]
}

@test "empty Bash command is denied" {
  run_gate "$(tool_json "Bash" '{"command":""}')"
  [ "$status" -eq 0 ]
  assert_decision "deny"
  assert_output_contains "Empty bash command"
}

@test "Write with relative path within project is allowed" {
  # Relative path — parent resolves to $CLAUDE_PROJECT_DIR which exists
  run_gate "$(tool_json "Write" '{"file_path":"main.sh"}')"
  [ "$status" -eq 0 ]
  assert_decision "allow"
}
