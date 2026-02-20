#!/usr/bin/env bash
# sipag — claude invocation helper

# Run claude with a prompt, returning its output on stdout.
# Arguments: prompt (the full prompt string)
# Environment:
#   SIPAG_SKIP_PERMISSIONS  Set to 0 for interactive mode (default: 1)
#   SIPAG_MODEL             Model override (e.g. claude-opus-4-5)
#   SIPAG_TIMEOUT           Timeout in seconds (default: 600)
#   SIPAG_CLAUDE_ARGS       Extra raw args appended to the claude invocation
run_claude() {
	local prompt="$1"

	local -a args=(--print)

	if [[ "${SIPAG_SKIP_PERMISSIONS:-1}" == "1" ]]; then
		args+=(--dangerously-skip-permissions)
	fi

	if [[ -n "${SIPAG_MODEL:-}" ]]; then
		args+=(--model "$SIPAG_MODEL")
	fi

	# Append any extra raw args supplied by the caller
	if [[ -n "${SIPAG_CLAUDE_ARGS:-}" ]]; then
		# shellcheck disable=SC2206
		args+=($SIPAG_CLAUDE_ARGS)
	fi

	timeout "${SIPAG_TIMEOUT:-600}" claude "${args[@]}" -p "$prompt"
}
