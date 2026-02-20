#!/usr/bin/env bash
# sipag — desktop notification helper

# Send a desktop notification.
# Usage: notify "title" "message"
# Set SIPAG_NOTIFY=0 to disable notifications (default: enabled).
notify() {
	local title="$1"
	local message="$2"

	# Allow users to opt out via env var (default: 1 = enabled)
	if [[ "${SIPAG_NOTIFY:-1}" == "0" ]]; then
		return 0
	fi

	if [[ "$(uname)" == "Darwin" ]]; then
		osascript -e "display notification \"${message}\" with title \"${title}\"" 2>/dev/null || true
	elif command -v notify-send &>/dev/null; then
		notify-send "${title}" "${message}" 2>/dev/null || true
	else
		# Fallback: terminal bell
		printf '\a'
	fi
}
