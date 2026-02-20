#!/usr/bin/env bats
# sipag — unit tests for lib/worker.sh

load ../helpers/test-helpers
load ../helpers/mock-commands

WORKER="${SIPAG_ROOT}/lib/worker.sh"

setup() {
	setup_common
	# Create a temporary ~/.sipag dir for config-file tests
	export SIPAG_DIR="${TEST_TMPDIR}/sipag"
	mkdir -p "$SIPAG_DIR"
	unset SIPAG_WORK_LABEL 2>/dev/null || true

	# Source the worker library (functions only; standalone guard prevents main)
	# shellcheck source=/dev/null
	source "$WORKER"
}

teardown() {
	teardown_common
}

# ─── worker_load_config: default label ───────────────────────────────────────

@test "worker_load_config: no config file leaves SIPAG_WORK_LABEL unset" {
	unset SIPAG_WORK_LABEL
	worker_load_config
	# After loading with no config file the var should still be unset (empty)
	[[ -z "${SIPAG_WORK_LABEL:-}" ]]
}

@test "worker_load_config: reads work_label from config file" {
	echo "work_label=needs-review" > "${SIPAG_DIR}/config"
	unset SIPAG_WORK_LABEL
	worker_load_config
	[[ "$SIPAG_WORK_LABEL" == "needs-review" ]]
}

@test "worker_load_config: env var takes precedence over config file" {
	echo "work_label=from-file" > "${SIPAG_DIR}/config"
	export SIPAG_WORK_LABEL="from-env"
	worker_load_config
	[[ "$SIPAG_WORK_LABEL" == "from-env" ]]
}

@test "worker_load_config: empty env var takes precedence over config file" {
	echo "work_label=from-file" > "${SIPAG_DIR}/config"
	export SIPAG_WORK_LABEL=""
	worker_load_config
	# Explicit empty string should win; no label filter
	[[ -z "$SIPAG_WORK_LABEL" ]]
}

@test "worker_load_config: ignores comment lines" {
	printf '# this is a comment\nwork_label=ready\n' > "${SIPAG_DIR}/config"
	unset SIPAG_WORK_LABEL
	worker_load_config
	[[ "$SIPAG_WORK_LABEL" == "ready" ]]
}

@test "worker_load_config: ignores blank lines" {
	printf '\n\nwork_label=ci-approved\n' > "${SIPAG_DIR}/config"
	unset SIPAG_WORK_LABEL
	worker_load_config
	[[ "$SIPAG_WORK_LABEL" == "ci-approved" ]]
}

@test "worker_load_config: unknown keys are silently ignored" {
	printf 'unknown_key=value\nwork_label=ok\n' > "${SIPAG_DIR}/config"
	unset SIPAG_WORK_LABEL
	worker_load_config
	[[ "$SIPAG_WORK_LABEL" == "ok" ]]
}

# ─── worker_loop: argument validation ────────────────────────────────────────

@test "worker_loop: exits non-zero when repo argument is missing" {
	run worker_loop
	[[ "$status" -ne 0 ]]
}

@test "worker_loop: prints error to stderr when repo is missing" {
	run worker_loop 2>&1
	[[ "$output" == *"Error"* ]]
}

# ─── worker_loop: label selection ────────────────────────────────────────────

@test "worker_loop: defaults to 'approved' label when nothing else is set" {
	unset SIPAG_WORK_LABEL
	# Mock gh to capture the --label argument and return an empty list
	create_mock "gh" 0 "[]"
	create_mock "jq" 0 "0"

	run worker_loop "owner/repo"

	local calls
	calls="$(get_mock_calls "gh")"
	[[ "$calls" == *"--label"* ]]
	[[ "$calls" == *"approved"* ]]
}

@test "worker_loop: uses SIPAG_WORK_LABEL env var when set" {
	export SIPAG_WORK_LABEL="greenlit"
	create_mock "gh" 0 "[]"
	create_mock "jq" 0 "0"

	run worker_loop "owner/repo"

	local calls
	calls="$(get_mock_calls "gh")"
	[[ "$calls" == *"greenlit"* ]]
}

@test "worker_loop: uses work_label from config file" {
	unset SIPAG_WORK_LABEL
	echo "work_label=config-label" > "${SIPAG_DIR}/config"
	create_mock "gh" 0 "[]"
	create_mock "jq" 0 "0"

	run worker_loop "owner/repo"

	local calls
	calls="$(get_mock_calls "gh")"
	[[ "$calls" == *"config-label"* ]]
}

@test "worker_loop: omits --label when SIPAG_WORK_LABEL is empty" {
	export SIPAG_WORK_LABEL=""
	create_mock "gh" 0 "[]"
	create_mock "jq" 0 "0"

	run worker_loop "owner/repo"

	local calls
	calls="$(get_mock_calls "gh")"
	[[ "$calls" != *"--label"* ]]
}

# ─── worker_loop: empty result handling ──────────────────────────────────────

@test "worker_loop: exits 0 when gh returns empty array" {
	create_mock "gh" 0 "[]"
	create_mock "jq" 0 "0"

	run worker_loop "owner/repo"
	[[ "$status" -eq 0 ]]
}

@test "worker_loop: prints 'No issues found' when gh returns empty array" {
	create_mock "gh" 0 "[]"
	create_mock "jq" 0 "0"

	run worker_loop "owner/repo"
	[[ "$output" == *"No issues found"* ]]
}

# ─── worker_loop: gh invocation ──────────────────────────────────────────────

@test "worker_loop: passes --state open to gh" {
	create_mock "gh" 0 "[]"
	create_mock "jq" 0 "0"

	run worker_loop "owner/repo"
	local calls
	calls="$(get_mock_calls "gh")"
	[[ "$calls" == *"--state"* ]]
	[[ "$calls" == *"open"* ]]
}

@test "worker_loop: passes --json number,title to gh" {
	create_mock "gh" 0 "[]"
	create_mock "jq" 0 "0"

	run worker_loop "owner/repo"
	local calls
	calls="$(get_mock_calls "gh")"
	[[ "$calls" == *"number,title"* ]]
}

@test "worker_loop: passes --repo argument to gh" {
	create_mock "gh" 0 "[]"
	create_mock "jq" 0 "0"

	run worker_loop "myorg/myrepo"
	local calls
	calls="$(get_mock_calls "gh")"
	[[ "$calls" == *"myorg/myrepo"* ]]
}
