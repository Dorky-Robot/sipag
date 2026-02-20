#!/usr/bin/env bash
# sipag — GitHub-issue-driven worker loop
#
# Fetches open issues from a GitHub repo filtered by a configurable label
# and dispatches each one to `sipag run`.
#
# Usage (standalone):
#   lib/worker.sh <owner/repo>
#
# Environment variables:
#   SIPAG_WORK_LABEL   Label to filter issues (default: "approved").
#                      Set to empty string to pick up ALL open issues.
#   SIPAG_DIR          sipag state directory (default: ~/.sipag)
#
# Requires: gh (GitHub CLI), jq

# worker_load_config
# Reads ~/.sipag/config (key=value format) and sets SIPAG_WORK_LABEL from the
# work_label key when the env var is not already set.
# Environment variables always take precedence over config file values.
worker_load_config() {
	local config_file="${SIPAG_DIR:-${HOME}/.sipag}/config"
	[[ -f "$config_file" ]] || return 0

	while IFS='=' read -r key value; do
		# Skip blank lines and comments
		[[ -z "$key" || "$key" =~ ^[[:space:]]*# ]] && continue
		# Trim surrounding whitespace
		key="${key#"${key%%[! ]*}"}"
		key="${key%"${key##*[! ]}"}"
		value="${value#"${value%%[! ]*}"}"
		value="${value%"${value##*[! ]}"}"
		[[ -z "$key" ]] && continue

		case "$key" in
		work_label)
			# Only set from config file when SIPAG_WORK_LABEL is not already exported.
			# Use indirect assignment so that an explicit export SIPAG_WORK_LABEL=""
			# (empty string) still wins over the config file value.
			if [[ -z "${SIPAG_WORK_LABEL+set}" ]]; then
				SIPAG_WORK_LABEL="$value"
				export SIPAG_WORK_LABEL
			fi
			;;
		esac
	done <"$config_file"
}

# worker_loop <owner/repo>
#
# Fetches open GitHub issues labelled with SIPAG_WORK_LABEL (default: "approved")
# from <owner/repo>, then dispatches each to `sipag run`.
#
# When SIPAG_WORK_LABEL is set to an empty string the label filter is omitted
# and ALL open issues are picked up (preserving pre-feature behaviour).
worker_loop() {
	local repo="$1"

	if [[ -z "$repo" ]]; then
		echo "Error: repo argument is required (owner/name)" >&2
		return 1
	fi

	# Populate SIPAG_WORK_LABEL from config if not set by env
	worker_load_config

	local work_label="${SIPAG_WORK_LABEL:-approved}"

	local gh_args=(
		issue list
		--repo "$repo"
		--state open
		--json number,title
		--limit 50
	)

	if [[ -n "$work_label" ]]; then
		gh_args+=(--label "$work_label")
		echo "sipag worker: fetching '$work_label' issues from $repo"
	else
		echo "sipag worker: fetching ALL open issues from $repo (no label filter)"
	fi

	local issues
	issues="$(gh "${gh_args[@]}")"

	if [[ -z "$issues" || "$issues" == "[]" ]]; then
		echo "No issues found${work_label:+ with label '$work_label'}"
		return 0
	fi

	local count
	count="$(printf '%s' "$issues" | jq 'length')"
	echo "Found $count issue(s) — dispatching..."

	while IFS= read -r issue_json; do
		local number title
		number="$(printf '%s' "$issue_json" | jq -r '.number')"
		title="$(printf '%s' "$issue_json" | jq -r '.title')"

		echo "==> Issue #$number: $title"
		sipag run --repo "https://github.com/$repo" --issue "$number" "$title"
	done < <(printf '%s' "$issues" | jq -c '.[]')
}

# Allow the script to be invoked directly as well as sourced.
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
	worker_loop "$@"
fi
