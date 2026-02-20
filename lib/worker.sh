#!/usr/bin/env bash
# sipag — GitHub-issue-driven worker loop
#
# Provides:
#   worker_load_config()  — load ~/.sipag/config into shell variables
#   worker_loop()         — fetch approved issues and run sipag for each
#
# Requires: gh (GitHub CLI), jq
#
# Environment variables:
#   SIPAG_WORK_LABEL  — label to filter issues on (default: approved)
#   SIPAG_DIR         — sipag state directory (default: ~/.sipag)
#   SIPAG_IMAGE       — Docker image for workers (default: sipag-worker:latest)
#   SIPAG_TIMEOUT     — per-task timeout in seconds (default: 1800)

# worker_load_config
# Reads ~/.sipag/config (key=value format) and exports recognised variables.
# Supported keys: work_label, image, timeout
# Environment variables take precedence over config-file values.
worker_load_config() {
	local sipag_dir="${SIPAG_DIR:-${HOME}/.sipag}"
	local config_file="${sipag_dir}/config"

	# Variables default values (used only when neither env nor config sets them)
	local _cfg_work_label="approved"
	local _cfg_image="sipag-worker:latest"
	local _cfg_timeout="1800"

	if [[ -f "$config_file" ]]; then
		while IFS='=' read -r key value || [[ -n "$key" ]]; do
			# Strip leading/trailing whitespace and skip comments/blank lines
			key="${key#"${key%%[![:space:]]*}"}"
			key="${key%"${key##*[![:space:]]}"}"
			[[ -z "$key" || "$key" == \#* ]] && continue

			value="${value#"${value%%[![:space:]]*}"}"
			value="${value%"${value##*[![:space:]]}"}"

			case "$key" in
			work_label) _cfg_work_label="$value" ;;
			image)      _cfg_image="$value" ;;
			timeout)    _cfg_timeout="$value" ;;
			esac
		done <"$config_file"
	fi

	# Env vars take precedence over config file values
	SIPAG_WORK_LABEL="${SIPAG_WORK_LABEL:-$_cfg_work_label}"
	SIPAG_IMAGE="${SIPAG_IMAGE:-$_cfg_image}"
	SIPAG_TIMEOUT="${SIPAG_TIMEOUT:-$_cfg_timeout}"

	export SIPAG_WORK_LABEL SIPAG_IMAGE SIPAG_TIMEOUT
}

# worker_loop <owner/repo> [<repo-url>]
#
# Fetches open GitHub issues labelled with SIPAG_WORK_LABEL (default:
# "approved") from <owner/repo>, then dispatches each to `sipag run`.
#
# Arguments:
#   $1  owner/repo  — GitHub repository slug (required)
#   $2  repo-url    — git clone URL; defaults to https://github.com/<owner/repo>
#
# When SIPAG_WORK_LABEL is empty the label filter is omitted and ALL open
# issues are picked up (preserving previous behaviour when explicitly opted in).
worker_loop() {
	local repo="${1:?worker_loop: owner/repo argument required}"
	local repo_url="${2:-https://github.com/${repo}}"

	worker_load_config

	local work_label="${SIPAG_WORK_LABEL}"

	if [[ -n "$work_label" ]]; then
		echo "==> Fetching open issues labelled '${work_label}' from ${repo}…"
	else
		echo "==> Fetching ALL open issues from ${repo} (no label filter)…"
	fi

	local gh_args=(
		issue list
		--repo "$repo"
		--state open
		--json number,title
		--limit 50
	)

	if [[ -n "$work_label" ]]; then
		gh_args+=(--label "$work_label")
	fi

	local issues
	issues=$(gh "${gh_args[@]}")

	local issue_count
	issue_count=$(printf '%s' "$issues" | jq 'length')
	echo "==> Found ${issue_count} issue(s)"

	if [[ "$issue_count" -eq 0 ]]; then
		echo "==> Nothing to work on."
		return 0
	fi

	local processed=0
	local failed=0

	printf '%s' "$issues" | jq -c '.[]' | while IFS= read -r issue; do
		local number title
		number=$(printf '%s' "$issue" | jq -r '.number')
		title=$(printf '%s' "$issue" | jq -r '.title')

		echo "==> Working on #${number}: ${title}"

		if sipag run \
			--repo "$repo_url" \
			--issue "$number" \
			--background \
			"$title"; then
			echo "==> Dispatched #${number}"
			processed=$((processed + 1))
		else
			echo "==> Failed to dispatch #${number}" >&2
			failed=$((failed + 1))
		fi
	done

	echo "==> worker_loop complete (dispatched=${processed} failed=${failed})"
}
