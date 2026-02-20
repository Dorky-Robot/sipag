#!/usr/bin/env bash
# sipag — claude invocation

# Run claude with the given task title and optional body.
# Respects env vars: SIPAG_PROMPT_PREFIX, SIPAG_SKIP_PERMISSIONS,
# SIPAG_MODEL, SIPAG_TIMEOUT, SIPAG_CLAUDE_ARGS.
run_claude() {
	local title="$1"
	local body="${2:-}"

	local prompt="${title}"
	if [[ -n "${SIPAG_PROMPT_PREFIX:-}" ]]; then
		prompt="${SIPAG_PROMPT_PREFIX}"$'\n\n'"${prompt}"
	fi
	if [[ -n "$body" ]]; then
		prompt+=$'\n\n'"${body}"
	fi

	local -a args=(--print)
	if [[ "${SIPAG_SKIP_PERMISSIONS:-1}" == "1" ]]; then
		args+=(--dangerously-skip-permissions)
	fi
	if [[ -n "${SIPAG_MODEL:-}" ]]; then
		args+=(--model "$SIPAG_MODEL")
	fi

	# Append any extra raw args
	if [[ -n "${SIPAG_CLAUDE_ARGS:-}" ]]; then
		# shellcheck disable=SC2206
		args+=($SIPAG_CLAUDE_ARGS)
	fi

	timeout "${SIPAG_TIMEOUT:-600}" claude "${args[@]}" -p "$prompt"
}
