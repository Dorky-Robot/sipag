#!/usr/bin/env bash
# sipag — Claude CLI wrapper

# Invoke Claude with a prompt and return the response on stdout.
# Arguments: prompt (string)
# Environment:
#   SIPAG_MODEL            — Claude model to use (optional)
#   SIPAG_SKIP_PERMISSIONS — set to 0 to keep permission prompts (default: 1)
#   SIPAG_TIMEOUT          — timeout in seconds (default: 600)
run_claude() {
	local prompt="$1"
	local skip_perms="${SIPAG_SKIP_PERMISSIONS:-1}"
	local timeout_secs="${SIPAG_TIMEOUT:-600}"
	local args
	args=("--print")

	if [[ "$skip_perms" == "1" ]]; then
		args+=("--dangerously-skip-permissions")
	fi

	if [[ -n "${SIPAG_MODEL:-}" ]]; then
		args+=("--model" "${SIPAG_MODEL}")
	fi

	args+=("-p" "$prompt")

	timeout "${timeout_secs}" claude "${args[@]}"
}
