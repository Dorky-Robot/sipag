#!/usr/bin/env bash
# sipag — local Claude runner
#
# Provides run_claude() for invoking Claude directly on the host (non-Docker).
# Respects the same environment variables as the Rust executor:
#   SIPAG_TIMEOUT           — timeout in seconds (default: 600)
#   SIPAG_SKIP_PERMISSIONS  — if "1", passes --dangerously-skip-permissions (default: 1)
#   SIPAG_MODEL             — model name to pass via --model (optional)
#   SIPAG_CLAUDE_ARGS       — extra whitespace-separated arguments (optional)

# run_claude <prompt>
# Runs Claude locally with the given prompt and prints its output to stdout.
# Exits with the claude process exit code.
run_claude() {
	local prompt="$1"
	local timeout_secs="${SIPAG_TIMEOUT:-600}"

	local -a claude_args=("--print")

	if [[ "${SIPAG_SKIP_PERMISSIONS:-1}" == "1" ]]; then
		claude_args+=("--dangerously-skip-permissions")
	fi

	if [[ -n "${SIPAG_MODEL:-}" ]]; then
		claude_args+=("--model" "$SIPAG_MODEL")
	fi

	# Split SIPAG_CLAUDE_ARGS on whitespace into individual arguments
	if [[ -n "${SIPAG_CLAUDE_ARGS:-}" ]]; then
		read -ra extra_args <<<"$SIPAG_CLAUDE_ARGS"
		claude_args+=("${extra_args[@]}")
	fi

	claude_args+=("-p" "$prompt")

	timeout "$timeout_secs" claude "${claude_args[@]}"
}
