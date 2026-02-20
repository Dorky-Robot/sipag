#!/usr/bin/env bats
# sipag — unit tests for lib/worker.sh

load ../helpers/test-helpers
load ../helpers/mock-commands

WORKER_SH="${SIPAG_ROOT}/lib/worker.sh"

setup() {
	setup_common
	unset SIPAG_WORK_LABEL SIPAG_DIR SIPAG_IMAGE SIPAG_TIMEOUT 2>/dev/null || true
}

teardown() {
	teardown_common
}

# ─── worker_load_config ───────────────────────────────────────────────────────

@test "worker_load_config: defaults work_label to 'approved' when no config" {
	# No ~/.sipag/config exists
	export SIPAG_DIR="${TEST_TMPDIR}/empty-sipag"
	mkdir -p "${SIPAG_DIR}"

	source "${WORKER_SH}"
	worker_load_config

	[[ "$SIPAG_WORK_LABEL" == "approved" ]]
}

@test "worker_load_config: reads work_label from config file" {
	export SIPAG_DIR="${TEST_TMPDIR}/sipag"
	mkdir -p "${SIPAG_DIR}"
	printf 'work_label=ready\n' > "${SIPAG_DIR}/config"

	source "${WORKER_SH}"
	worker_load_config

	[[ "$SIPAG_WORK_LABEL" == "ready" ]]
}

@test "worker_load_config: env var SIPAG_WORK_LABEL takes precedence over config" {
	export SIPAG_DIR="${TEST_TMPDIR}/sipag"
	mkdir -p "${SIPAG_DIR}"
	printf 'work_label=from-config\n' > "${SIPAG_DIR}/config"
	export SIPAG_WORK_LABEL="from-env"

	source "${WORKER_SH}"
	worker_load_config

	[[ "$SIPAG_WORK_LABEL" == "from-env" ]]
}

@test "worker_load_config: empty work_label in config disables label filter" {
	export SIPAG_DIR="${TEST_TMPDIR}/sipag"
	mkdir -p "${SIPAG_DIR}"
	printf 'work_label=\n' > "${SIPAG_DIR}/config"

	source "${WORKER_SH}"
	worker_load_config

	[[ -z "$SIPAG_WORK_LABEL" ]]
}

@test "worker_load_config: ignores comment lines in config" {
	export SIPAG_DIR="${TEST_TMPDIR}/sipag"
	mkdir -p "${SIPAG_DIR}"
	cat > "${SIPAG_DIR}/config" <<'EOF'
# This is a comment
work_label=approved
# Another comment
EOF

	source "${WORKER_SH}"
	worker_load_config

	[[ "$SIPAG_WORK_LABEL" == "approved" ]]
}

@test "worker_load_config: ignores unrecognised keys in config" {
	export SIPAG_DIR="${TEST_TMPDIR}/sipag"
	mkdir -p "${SIPAG_DIR}"
	printf 'unknown_key=value\nwork_label=approved\n' > "${SIPAG_DIR}/config"

	source "${WORKER_SH}"
	worker_load_config

	[[ "$SIPAG_WORK_LABEL" == "approved" ]]
}

# ─── worker_loop ─────────────────────────────────────────────────────────────

@test "worker_loop: calls gh issue list with --label approved by default" {
	export SIPAG_DIR="${TEST_TMPDIR}/sipag"
	mkdir -p "${SIPAG_DIR}"

	# Mock gh: return one approved issue
	cat > "${TEST_TMPDIR}/bin/gh" <<'ENDMOCK'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "${TEST_TMPDIR}/mock-calls/gh"
printf '[{"number":42,"title":"Fix the bug"}]'
ENDMOCK
	chmod +x "${TEST_TMPDIR}/bin/gh"
	# Make TEST_TMPDIR available inside the mock
	export TEST_TMPDIR

	# Mock sipag: record calls
	create_mock sipag 0 ""

	source "${WORKER_SH}"
	run worker_loop "owner/repo"

	[ "$status" -eq 0 ]

	# gh should have been called with --label approved
	local gh_calls
	gh_calls="$(cat "${TEST_TMPDIR}/mock-calls/gh" 2>/dev/null || echo "")"
	[[ "$gh_calls" == *"--label"* ]]
	[[ "$gh_calls" == *"approved"* ]]
}

@test "worker_loop: uses SIPAG_WORK_LABEL env var when set" {
	export SIPAG_DIR="${TEST_TMPDIR}/sipag"
	mkdir -p "${SIPAG_DIR}"
	export SIPAG_WORK_LABEL="ready-for-dev"

	cat > "${TEST_TMPDIR}/bin/gh" <<'ENDMOCK'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "${TEST_TMPDIR}/mock-calls/gh"
printf '[{"number":7,"title":"Implement feature"}]'
ENDMOCK
	chmod +x "${TEST_TMPDIR}/bin/gh"
	export TEST_TMPDIR

	create_mock sipag 0 ""

	source "${WORKER_SH}"
	run worker_loop "owner/repo"

	[ "$status" -eq 0 ]

	local gh_calls
	gh_calls="$(cat "${TEST_TMPDIR}/mock-calls/gh" 2>/dev/null || echo "")"
	[[ "$gh_calls" == *"ready-for-dev"* ]]
}

@test "worker_loop: omits --label when SIPAG_WORK_LABEL is empty" {
	export SIPAG_DIR="${TEST_TMPDIR}/sipag"
	mkdir -p "${SIPAG_DIR}"
	export SIPAG_WORK_LABEL=""

	cat > "${TEST_TMPDIR}/bin/gh" <<'ENDMOCK'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "${TEST_TMPDIR}/mock-calls/gh"
printf '[]'
ENDMOCK
	chmod +x "${TEST_TMPDIR}/bin/gh"
	export TEST_TMPDIR

	source "${WORKER_SH}"
	run worker_loop "owner/repo"

	[ "$status" -eq 0 ]

	local gh_calls
	gh_calls="$(cat "${TEST_TMPDIR}/mock-calls/gh" 2>/dev/null || echo "")"
	[[ "$gh_calls" != *"--label"* ]]
}

@test "worker_loop: exits cleanly when no issues found" {
	export SIPAG_DIR="${TEST_TMPDIR}/sipag"
	mkdir -p "${SIPAG_DIR}"

	cat > "${TEST_TMPDIR}/bin/gh" <<'ENDMOCK'
#!/usr/bin/env bash
printf '[]'
ENDMOCK
	chmod +x "${TEST_TMPDIR}/bin/gh"

	source "${WORKER_SH}"
	run worker_loop "owner/repo"

	[ "$status" -eq 0 ]
	[[ "$output" == *"Nothing to work on"* ]]
}

@test "worker_loop: dispatches sipag run for each issue" {
	export SIPAG_DIR="${TEST_TMPDIR}/sipag"
	mkdir -p "${SIPAG_DIR}"

	cat > "${TEST_TMPDIR}/bin/gh" <<'ENDMOCK'
#!/usr/bin/env bash
printf '[{"number":1,"title":"First"},{"number":2,"title":"Second"}]'
ENDMOCK
	chmod +x "${TEST_TMPDIR}/bin/gh"

	create_mock sipag 0 ""

	source "${WORKER_SH}"
	run worker_loop "owner/repo"

	[ "$status" -eq 0 ]

	# sipag should have been called twice
	local call_count
	call_count="$(mock_call_count sipag)"
	[[ "$call_count" -eq 2 ]]
}

@test "worker_loop: passes issue number to sipag run" {
	export SIPAG_DIR="${TEST_TMPDIR}/sipag"
	mkdir -p "${SIPAG_DIR}"

	cat > "${TEST_TMPDIR}/bin/gh" <<'ENDMOCK'
#!/usr/bin/env bash
printf '[{"number":42,"title":"Fix the bug"}]'
ENDMOCK
	chmod +x "${TEST_TMPDIR}/bin/gh"

	create_mock sipag 0 ""

	source "${WORKER_SH}"
	run worker_loop "owner/repo"

	[ "$status" -eq 0 ]

	local calls
	calls="$(get_mock_calls sipag)"
	[[ "$calls" == *"--issue"* ]]
	[[ "$calls" == *"42"* ]]
}

@test "worker_loop: defaults repo-url to https://github.com/<owner/repo>" {
	export SIPAG_DIR="${TEST_TMPDIR}/sipag"
	mkdir -p "${SIPAG_DIR}"

	cat > "${TEST_TMPDIR}/bin/gh" <<'ENDMOCK'
#!/usr/bin/env bash
printf '[{"number":1,"title":"Task"}]'
ENDMOCK
	chmod +x "${TEST_TMPDIR}/bin/gh"

	create_mock sipag 0 ""

	source "${WORKER_SH}"
	run worker_loop "myorg/myrepo"

	[ "$status" -eq 0 ]

	local calls
	calls="$(get_mock_calls sipag)"
	[[ "$calls" == *"https://github.com/myorg/myrepo"* ]]
}

@test "worker_loop: uses custom repo-url when provided" {
	export SIPAG_DIR="${TEST_TMPDIR}/sipag"
	mkdir -p "${SIPAG_DIR}"

	cat > "${TEST_TMPDIR}/bin/gh" <<'ENDMOCK'
#!/usr/bin/env bash
printf '[{"number":1,"title":"Task"}]'
ENDMOCK
	chmod +x "${TEST_TMPDIR}/bin/gh"

	create_mock sipag 0 ""

	source "${WORKER_SH}"
	run worker_loop "myorg/myrepo" "git@github.com:myorg/myrepo.git"

	[ "$status" -eq 0 ]

	local calls
	calls="$(get_mock_calls sipag)"
	[[ "$calls" == *"git@github.com:myorg/myrepo.git"* ]]
}

@test "worker_loop: reads work_label from config file" {
	export SIPAG_DIR="${TEST_TMPDIR}/sipag"
	mkdir -p "${SIPAG_DIR}"
	printf 'work_label=needs-work\n' > "${SIPAG_DIR}/config"
	unset SIPAG_WORK_LABEL

	cat > "${TEST_TMPDIR}/bin/gh" <<'ENDMOCK'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "${TEST_TMPDIR}/mock-calls/gh"
printf '[]'
ENDMOCK
	chmod +x "${TEST_TMPDIR}/bin/gh"
	export TEST_TMPDIR

	source "${WORKER_SH}"
	run worker_loop "owner/repo"

	[ "$status" -eq 0 ]

	local gh_calls
	gh_calls="$(cat "${TEST_TMPDIR}/mock-calls/gh" 2>/dev/null || echo "")"
	[[ "$gh_calls" == *"needs-work"* ]]
}

@test "worker_loop: fails with error when no repo argument given" {
	source "${WORKER_SH}"
	run worker_loop

	[ "$status" -ne 0 ]
}
