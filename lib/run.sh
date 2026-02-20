#!/usr/bin/env bash
# sipag — Claude runner helper

# Run claude with the given prompt and print output to stdout.
# Arguments: prompt
# Environment:
#   SIPAG_MODEL             — override the Claude model
#   SIPAG_SKIP_PERMISSIONS  — set to 0 to disable --dangerously-skip-permissions (default: 1)
#   SIPAG_TIMEOUT           — timeout in seconds (default: 600)
run_claude() {
	local prompt="$1"
	local args
	args=("--print")

	if [[ "${SIPAG_SKIP_PERMISSIONS:-1}" == "1" ]]; then
		args+=("--dangerously-skip-permissions")
	fi

	if [[ -n "${SIPAG_MODEL:-}" ]]; then
		args+=("--model" "$SIPAG_MODEL")
	fi

	args+=("-p" "$prompt")

	timeout "${SIPAG_TIMEOUT:-600}" claude "${args[@]}"
}
