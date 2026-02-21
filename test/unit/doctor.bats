#!/usr/bin/env bats
# sipag — unit tests for lib/doctor.sh

load ../helpers/test-helpers
load ../helpers/mock-commands

setup() {
	setup_common
	source "${SIPAG_ROOT}/lib/doctor.sh"

	# Redirect HOME so tests never touch the real user's settings
	export HOME="${TEST_TMPDIR}/home"
	mkdir -p "$HOME"

	# Use a temp sipag dir
	export SIPAG_DIR="${TEST_TMPDIR}/sipag"
	mkdir -p "${SIPAG_DIR}"

	# Create queue directories by default
	mkdir -p "${SIPAG_DIR}/queue" "${SIPAG_DIR}/running" "${SIPAG_DIR}/done" "${SIPAG_DIR}/failed"

	# Create a non-empty token file by default
	printf "fake-token\n" >"${SIPAG_DIR}/token"

	# Create claude settings with required permissions by default
	mkdir -p "${HOME}/.claude"
	cat >"${HOME}/.claude/settings.json" <<'JSON'
{
  "permissions": {
    "allow": [
      "Bash(gh issue *)",
      "Bash(gh pr *)",
      "Bash(gh label *)"
    ]
  }
}
JSON

	# Mock gh as present and authenticated
	create_mock "gh" 0 ""

	# Mock claude as present
	create_mock "claude" 0 "1.2.3"

	# Mock docker as present, daemon running, image found
	cat >"${TEST_TMPDIR}/bin/docker" <<'MOCK'
#!/usr/bin/env bash
case "$1" in
  --version) printf "Docker version 24.0.7, build afdd53b\n"; exit 0 ;;
  info)      exit 0 ;;
  image)
    shift
    if [[ "$1" == "inspect" ]]; then exit 0; fi
    ;;
esac
exit 0
MOCK
	chmod +x "${TEST_TMPDIR}/bin/docker"

	# Mock jq as present
	create_mock "jq" 0 "jq-1.7"

	# Unset API key by default
	unset ANTHROPIC_API_KEY || true
}

teardown() {
	teardown_common
}

# --- Happy path ---

@test "doctor_run: passes when everything is set up correctly" {
	run doctor_run
	[[ "$status" -eq 0 ]]
	assert_output_contains "All checks passed. Ready to go."
}

@test "doctor_run: shows OK for gh when installed" {
	run doctor_run
	assert_output_contains "OK  gh CLI"
}

@test "doctor_run: shows OK for claude when installed" {
	run doctor_run
	assert_output_contains "OK  claude CLI"
}

@test "doctor_run: shows OK for docker when installed" {
	run doctor_run
	assert_output_contains "OK  docker"
}

@test "doctor_run: shows OK for jq when installed" {
	run doctor_run
	assert_output_contains "OK  jq"
}

@test "doctor_run: shows OK for GitHub auth when authenticated" {
	run doctor_run
	assert_output_contains "OK  GitHub authenticated"
}

@test "doctor_run: shows OK for OAuth token when present" {
	run doctor_run
	assert_output_contains "OK  Claude OAuth token"
}

@test "doctor_run: shows OK for Docker daemon running" {
	run doctor_run
	assert_output_contains "OK  Docker daemon running"
}

@test "doctor_run: shows OK for sipag directory" {
	run doctor_run
	assert_output_contains "OK  ~/.sipag/ directory exists"
}

@test "doctor_run: shows OK for queue directories" {
	run doctor_run
	assert_output_contains "OK  Queue directories exist"
}

@test "doctor_run: shows OK for Claude Code permissions" {
	run doctor_run
	assert_output_contains "OK  Claude Code permissions configured"
}

# --- Missing tools ---

@test "doctor_run: ERR and exit 1 when gh is missing" {
	rm -f "${TEST_TMPDIR}/bin/gh"
	PATH="${TEST_TMPDIR}/bin" run doctor_run
	[[ "$status" -ne 0 ]]
	assert_output_contains "ERR gh not found"
}

@test "doctor_run: shows brew fix for gh on missing" {
	rm -f "${TEST_TMPDIR}/bin/gh"
	PATH="${TEST_TMPDIR}/bin" run doctor_run
	assert_output_contains "brew install gh"
}

@test "doctor_run: ERR and exit 1 when claude is missing" {
	rm -f "${TEST_TMPDIR}/bin/claude"
	PATH="${TEST_TMPDIR}/bin" run doctor_run
	[[ "$status" -ne 0 ]]
	assert_output_contains "ERR claude not found"
}

@test "doctor_run: shows install URL for claude on missing" {
	rm -f "${TEST_TMPDIR}/bin/claude"
	PATH="${TEST_TMPDIR}/bin" run doctor_run
	assert_output_contains "claude.ai/code"
}

@test "doctor_run: ERR and exit 1 when docker is missing" {
	rm -f "${TEST_TMPDIR}/bin/docker"
	PATH="${TEST_TMPDIR}/bin" run doctor_run
	[[ "$status" -ne 0 ]]
	assert_output_contains "ERR docker not found"
}

@test "doctor_run: shows brew and docs.docker.com fix for docker on missing" {
	rm -f "${TEST_TMPDIR}/bin/docker"
	PATH="${TEST_TMPDIR}/bin" run doctor_run
	assert_output_contains "brew install --cask docker"
	assert_output_contains "docs.docker.com"
}

@test "doctor_run: WARN (not ERR) when jq is missing" {
	rm -f "${TEST_TMPDIR}/bin/jq"
	PATH="${TEST_TMPDIR}/bin" run doctor_run
	assert_output_contains "WARN jq not found"
	assert_output_not_contains "ERR jq"
}

@test "doctor_run: exit 0 when only jq is missing (warning only)" {
	rm -f "${TEST_TMPDIR}/bin/jq"
	PATH="${TEST_TMPDIR}/bin" run doctor_run
	[[ "$status" -eq 0 ]]
}

# --- Authentication errors ---

@test "doctor_run: ERR when GitHub not authenticated" {
	create_mock "gh" 1 ""
	run doctor_run
	[[ "$status" -ne 0 ]]
	assert_output_contains "ERR GitHub not authenticated"
}

@test "doctor_run: shows gh auth login fix for unauthenticated GitHub" {
	create_mock "gh" 1 ""
	run doctor_run
	assert_output_contains "gh auth login"
}

@test "doctor_run: ERR when OAuth token missing" {
	rm -f "${SIPAG_DIR}/token"
	run doctor_run
	[[ "$status" -ne 0 ]]
	assert_output_contains "ERR Claude OAuth token missing"
}

@test "doctor_run: ERR when OAuth token is empty file" {
	printf "" >"${SIPAG_DIR}/token"
	run doctor_run
	[[ "$status" -ne 0 ]]
	assert_output_contains "ERR Claude OAuth token missing"
}

@test "doctor_run: shows claude setup-token fix when token missing" {
	rm -f "${SIPAG_DIR}/token"
	run doctor_run
	assert_output_contains "claude setup-token"
	assert_output_contains "cp ~/.claude/token"
}

@test "doctor_run: explains what the fix commands do when token missing" {
	rm -f "${SIPAG_DIR}/token"
	run doctor_run
	assert_output_contains "opens your browser"
}

@test "doctor_run: mentions API key as alternative when token missing" {
	rm -f "${SIPAG_DIR}/token"
	run doctor_run
	assert_output_contains "ANTHROPIC_API_KEY"
}

@test "doctor_run: info line for ANTHROPIC_API_KEY when not set" {
	run doctor_run
	assert_output_contains "ANTHROPIC_API_KEY not set (optional"
}

@test "doctor_run: info line for ANTHROPIC_API_KEY when set" {
	ANTHROPIC_API_KEY="sk-ant-test" run doctor_run
	assert_output_contains "ANTHROPIC_API_KEY set (optional"
}

# --- Docker errors ---

@test "doctor_run: ERR when Docker daemon not running" {
	cat >"${TEST_TMPDIR}/bin/docker" <<'MOCK'
#!/usr/bin/env bash
case "$1" in
  --version) printf "Docker version 24.0.7, build afdd53b\n"; exit 0 ;;
  info)      exit 1 ;;
  image)     exit 0 ;;
esac
exit 0
MOCK
	chmod +x "${TEST_TMPDIR}/bin/docker"
	run doctor_run
	[[ "$status" -ne 0 ]]
	assert_output_contains "ERR Docker daemon not running"
}

@test "doctor_run: shows Docker Desktop / systemctl fix when daemon not running" {
	cat >"${TEST_TMPDIR}/bin/docker" <<'MOCK'
#!/usr/bin/env bash
case "$1" in
  --version) printf "Docker version 24.0.7, build afdd53b\n"; exit 0 ;;
  info)      exit 1 ;;
  image)     exit 0 ;;
esac
exit 0
MOCK
	chmod +x "${TEST_TMPDIR}/bin/docker"
	run doctor_run
	assert_output_contains "Docker Desktop"
	assert_output_contains "systemctl start docker"
}

@test "doctor_run: ERR when worker image not found" {
	cat >"${TEST_TMPDIR}/bin/docker" <<'MOCK'
#!/usr/bin/env bash
case "$1" in
  --version) printf "Docker version 24.0.7, build afdd53b\n"; exit 0 ;;
  info)      exit 0 ;;
  image)
    shift
    if [[ "$1" == "inspect" ]]; then exit 1; fi
    ;;
esac
exit 0
MOCK
	chmod +x "${TEST_TMPDIR}/bin/docker"
	run doctor_run
	[[ "$status" -ne 0 ]]
	assert_output_contains "ERR"
	assert_output_contains "image not found"
}

@test "doctor_run: shows sipag setup fix when image not found" {
	cat >"${TEST_TMPDIR}/bin/docker" <<'MOCK'
#!/usr/bin/env bash
case "$1" in
  --version) printf "Docker version 24.0.7, build afdd53b\n"; exit 0 ;;
  info)      exit 0 ;;
  image)
    shift
    if [[ "$1" == "inspect" ]]; then exit 1; fi
    ;;
esac
exit 0
MOCK
	chmod +x "${TEST_TMPDIR}/bin/docker"
	run doctor_run
	assert_output_contains "sipag setup"
}

@test "doctor_run: skips docker daemon check when docker not installed" {
	rm -f "${TEST_TMPDIR}/bin/docker"
	PATH="${TEST_TMPDIR}/bin" run doctor_run
	assert_output_contains "Docker checks skipped"
}

# --- sipag directory errors ---

@test "doctor_run: ERR when ~/.sipag directory missing" {
	rm -rf "${SIPAG_DIR}"
	run doctor_run
	[[ "$status" -ne 0 ]]
	assert_output_contains "ERR ~/.sipag/ directory missing"
}

@test "doctor_run: shows sipag setup fix when sipag dir missing" {
	rm -rf "${SIPAG_DIR}"
	run doctor_run
	assert_output_contains "sipag setup"
}

@test "doctor_run: ERR when queue directories missing" {
	rm -rf "${SIPAG_DIR}/queue" "${SIPAG_DIR}/running"
	run doctor_run
	[[ "$status" -ne 0 ]]
	assert_output_contains "ERR Queue directories missing"
}

@test "doctor_run: names missing queue dirs in error" {
	rm -rf "${SIPAG_DIR}/queue"
	run doctor_run
	assert_output_contains "queue"
}

# --- Claude Code permissions ---

@test "doctor_run: ERR when Claude Code permissions missing" {
	printf '{"permissions":{"allow":[]}}\n' >"${HOME}/.claude/settings.json"
	run doctor_run
	[[ "$status" -ne 0 ]]
	assert_output_contains "ERR Claude Code permissions missing"
}

@test "doctor_run: shows missing permission names when permissions incomplete" {
	printf '{"permissions":{"allow":[]}}\n' >"${HOME}/.claude/settings.json"
	run doctor_run
	assert_output_contains "Bash(gh issue *)"
}

@test "doctor_run: shows sipag setup fix when permissions missing" {
	printf '{"permissions":{"allow":[]}}\n' >"${HOME}/.claude/settings.json"
	run doctor_run
	assert_output_contains "sipag setup"
}

@test "doctor_run: ERR when claude settings file does not exist" {
	rm -f "${HOME}/.claude/settings.json"
	run doctor_run
	[[ "$status" -ne 0 ]]
	assert_output_contains "ERR Claude Code permissions missing"
}

# --- Summary lines ---

@test "doctor_run: summary lists error count when errors found" {
	rm -f "${SIPAG_DIR}/token"
	run doctor_run
	assert_output_contains "error(s)"
}

@test "doctor_run: summary mentions sipag setup when errors found" {
	rm -f "${SIPAG_DIR}/token"
	run doctor_run
	assert_output_contains "sipag setup"
}

@test "doctor_run: summary shows warning count when only warnings" {
	rm -f "${TEST_TMPDIR}/bin/jq"
	PATH="${TEST_TMPDIR}/bin" run doctor_run
	assert_output_contains "warning(s)"
}

@test "doctor_run: outputs section headers" {
	run doctor_run
	assert_output_contains "Core tools:"
	assert_output_contains "Authentication:"
	assert_output_contains "Docker:"
	assert_output_contains "sipag:"
}

@test "doctor_run: outputs banner" {
	run doctor_run
	assert_output_contains "=== sipag doctor ==="
}
