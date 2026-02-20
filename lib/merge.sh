#!/usr/bin/env bash
# sipag — merge approved PRs (no Claude)
#
# Provides merge_run() which finds PRs that have been approved and merges
# them serially using rebase strategy.
#
# Requires: gh (GitHub CLI), jq

# merge_run <owner/repo>
# Lists PRs with an approved review state and no pending change requests,
# then merges each one serially with --rebase.
merge_run() {
	local repo="$1"

	if [[ -z "$repo" ]]; then
		echo "merge_run: repository argument required (format: owner/repo)" >&2
		return 1
	fi

	# Validate repo format
	if [[ "$repo" != */* ]]; then
		echo "Error: repo must be in owner/repo format (got: $repo)" >&2
		return 1
	fi

	echo "==> Fetching open PRs for ${repo}…"

	# Get open PRs with their review decision
	local prs
	prs="$(gh pr list \
		--repo "$repo" \
		--state open \
		--json number,title,reviewDecision \
		--jq '.[] | select(.reviewDecision == "APPROVED") | .number')" || {
		echo "Error: failed to list PRs for $repo" >&2
		return 1
	}

	if [[ -z "$prs" ]]; then
		echo "==> No approved PRs to merge."
		return 0
	fi

	local pr_count
	pr_count="$(echo "$prs" | wc -l | tr -d ' ')"
	echo "==> Found ${pr_count} approved PR(s) to merge"

	local merged=0
	local failed=0

	while IFS= read -r pr_num; do
		[[ -z "$pr_num" ]] && continue

		echo ""
		echo "==> Merging PR #${pr_num} (rebase)…"

		if gh pr merge "$pr_num" \
			--repo "$repo" \
			--rebase \
			--delete-branch; then
			echo "    Merged #${pr_num}"
			(( merged++ )) || true
		else
			echo "Warning: failed to merge PR #${pr_num} — skipping" >&2
			(( failed++ )) || true
		fi
	done <<< "$prs"

	echo ""
	echo "==> Merge complete: ${merged} merged, ${failed} failed"

	if [[ "$failed" -gt 0 ]]; then
		return 1
	fi
	return 0
}
