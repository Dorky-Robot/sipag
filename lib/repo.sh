#!/usr/bin/env bash
# sipag — repo registry

# Look up a repo name in repos.conf and print the URL.
# Returns 1 if not found.
# Uses ${SIPAG_DIR:-~/.sipag}/repos.conf (one "name=url" entry per line).
repo_url() {
	local name="$1"
	local conf="${SIPAG_DIR:-${HOME}/.sipag}/repos.conf"

	if [[ ! -f "$conf" ]]; then
		return 1
	fi

	local line key val
	while IFS= read -r line || [[ -n "$line" ]]; do
		key="${line%%=*}"
		val="${line#*=}"
		if [[ "$key" == "$name" ]]; then
			echo "$val"
			return 0
		fi
	done <"$conf"

	return 1
}
