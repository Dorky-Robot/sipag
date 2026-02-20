#!/usr/bin/env bash
# sipag — desktop notification helper

# Send a desktop notification when a task completes.
# Arguments: status (success|failure) title
# Environment: SIPAG_NOTIFY — set to 0 to disable notifications (default: enabled)
notify() {
	local status="$1"
	local title="$2"

	# Check if disabled
	if [[ "${SIPAG_NOTIFY:-1}" == "0" ]]; then
		return 0
	fi

	local msg
	if [[ "$status" == "success" ]]; then
		msg="sipag: ✓ ${title} — PR ready for review"
	else
		msg="sipag: ✗ ${title} — check logs"
	fi

	if [[ "$(uname)" == "Darwin" ]]; then
		osascript -e "display notification \"${msg}\" with title \"sipag\"" 2>/dev/null || true
	elif command -v notify-send >/dev/null 2>&1; then
		notify-send "sipag" "${msg}" 2>/dev/null || true
	else
		printf '\a'
	fi
}
