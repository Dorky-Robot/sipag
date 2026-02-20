#!/usr/bin/env bash
# sipag — merge approved PRs
#
# Provides: merge_run <owner/repo>
# Finds all approved, open PRs for the repo and merges them with squash.

merge_run() {
	local repo="$1"

	printf '==> Checking for approved PRs in %s\n' "$repo"

	local prs
	prs=$(gh pr list \
		--repo "$repo" \
		--state open \
		--json number,title,reviewDecision \
		--jq '.[] | select(.reviewDecision == "APPROVED") | "\(.number)\t\(.title)"' \
		2>&1) || {
		printf 'Error: failed to list PRs for %s: %s\n' "$repo" "$prs" >&2
		return 1
	}

	if [[ -z "$prs" ]]; then
		printf 'No approved PRs found in %s\n' "$repo"
		return 0
	fi

	local merged=0
	while IFS=$'\t' read -r number title; do
		printf '==> Merging PR #%s: %s\n' "$number" "$title"
		if gh pr merge "$number" \
			--repo "$repo" \
			--squash \
			--delete-branch; then
			printf '==> Merged PR #%s\n' "$number"
			merged=$(( merged + 1 ))
		else
			printf 'Error: failed to merge PR #%s\n' "$number" >&2
		fi
	done <<<"$prs"

	printf '==> Done: merged %d PR(s)\n' "$merged"
}
