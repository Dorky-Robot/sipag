#!/usr/bin/env bash
# sipag — PR review helper using Claude

# Review all open pull requests in a GitHub repository.
# Arguments: repo (owner/repo format)
# Requires: gh, claude (via run_claude from lib/run.sh), jq
review_run() {
	local repo="$1"

	if [[ -z "$repo" ]]; then
		echo "Usage: sipag review <owner/repo>" >&2
		return 1
	fi

	# Validate repo format
	if [[ "$repo" != */* ]]; then
		echo "Error: repo must be in owner/repo format (got: $repo)" >&2
		return 1
	fi

	echo "==> Fetching open PRs for $repo"

	# Get list of open PR numbers
	local prs
	prs="$(gh pr list --repo "$repo" --state open --json number -q '.[].number' 2>&1)" || {
		echo "Error: failed to list PRs for $repo: $prs" >&2
		return 1
	}

	if [[ -z "$prs" ]]; then
		echo "No open pull requests in $repo"
		return 0
	fi

	local pr_count
	pr_count="$(echo "$prs" | wc -l | tr -d ' ')"
	echo "==> Found $pr_count open PR(s)"

	local reviewed=0
	local failed=0

	while IFS= read -r pr_num; do
		[[ -z "$pr_num" ]] && continue

		echo ""
		echo "==> Reviewing PR #$pr_num"

		# Fetch PR metadata
		local metadata
		metadata="$(gh pr view "$pr_num" --repo "$repo" --json title,body,files,comments 2>&1)" || {
			echo "Warning: failed to fetch metadata for PR #$pr_num — skipping" >&2
			(( failed++ )) || true
			continue
		}

		# Fetch diff
		local diff
		diff="$(gh pr diff "$pr_num" --repo "$repo" 2>&1)" || {
			echo "Warning: failed to fetch diff for PR #$pr_num — skipping" >&2
			(( failed++ )) || true
			continue
		}

		# Build prompt for Claude
		local prompt
		prompt="$(cat <<PROMPT
You are a senior software engineer performing a code review on a GitHub pull request.

## PR Metadata
${metadata}

## Diff
${diff}

## Review Instructions
- Approve if the code is correct and no significant issues found
- Request changes for bugs, security issues, or missing tests
- Be specific about what needs to change
- Output valid JSON only, no markdown fences

Output exactly this JSON format:
{
  "verdict": "approve" | "request_changes" | "comment",
  "summary": "one-line summary",
  "body": "overall review comment"
}
PROMPT
)"

		# Call Claude and capture output
		local response
		if ! response="$(run_claude "$prompt")"; then
			echo "Warning: claude failed for PR #$pr_num — skipping" >&2
			(( failed++ )) || true
			continue
		fi

		# Parse verdict and body from JSON response
		local verdict body
		verdict="$(echo "$response" | jq -r '.verdict // empty' 2>/dev/null)"
		body="$(echo "$response" | jq -r '.body // empty' 2>/dev/null)"

		if [[ -z "$verdict" ]]; then
			echo "Warning: could not parse Claude response for PR #$pr_num — skipping" >&2
			echo "Response was: $response" >&2
			(( failed++ )) || true
			continue
		fi

		echo "==> PR #$pr_num verdict: $verdict"

		# Apply the review via gh pr review
		case "$verdict" in
			approve)
				gh pr review "$pr_num" --repo "$repo" --approve --body "$body"
				;;
			request_changes)
				gh pr review "$pr_num" --repo "$repo" --request-changes --body "$body"
				;;
			comment)
				gh pr review "$pr_num" --repo "$repo" --comment --body "$body"
				;;
			*)
				echo "Warning: unknown verdict '$verdict' for PR #$pr_num — skipping" >&2
				(( failed++ )) || true
				continue
				;;
		esac

		(( reviewed++ )) || true
	done <<< "$prs"

	echo ""
	echo "==> Review complete: $reviewed reviewed, $failed skipped"

	if [[ "$failed" -gt 0 ]]; then
		return 1
	fi
	return 0
}
