#!/usr/bin/env bash
# sipag — GitHub issue triage via Claude
#
# Provides triage_run() which fetches open issues, asks Claude to classify
# them, then applies each action mechanically via the GitHub CLI.
#
# Requires: gh (GitHub CLI), jq, claude

# triage_run <owner/repo>
# Fetches up to 100 open issues, sends them to Claude for analysis, and
# applies the resulting actions (label / close / comment / none).
triage_run() {
	local repo="$1"

	if [[ -z "$repo" ]]; then
		echo "triage_run: repository argument required (format: owner/repo)" >&2
		return 1
	fi

	echo "==> Fetching open issues for ${repo}…"
	local issues
	issues=$(gh issue list \
		--repo "$repo" \
		--state open \
		--json number,title,body,labels,comments,createdAt \
		--limit 100)

	local issue_count
	issue_count=$(echo "$issues" | jq 'length')
	echo "==> Found ${issue_count} open issue(s)"

	if [[ "$issue_count" -eq 0 ]]; then
		echo "==> Nothing to triage."
		return 0
	fi

	local prompt
	prompt="$(
		cat <<PROMPT
You are triaging GitHub issues for the repository ${repo}.

Analyze each issue below and decide exactly one action per issue:

  label   — The issue is clear and actionable. Add a category label and a
             priority label. Category labels: bug, enhancement, refactor,
             docs, test. Priority labels: P0 (critical/blocker), P1 (high),
             P2 (medium), P3 (nice-to-have). Include ALL applicable labels.

  close   — The issue is a duplicate of another open issue, or has already
             been resolved by a merged PR. Provide a brief comment explaining
             why it is being closed.

  comment — The issue lacks enough information to act on. Post a short,
             specific clarifying question. Only use this when truly necessary.

  none    — No action is needed (e.g. already fully labelled, waiting on
             upstream, etc.).

Rules:
- Every issue must appear exactly once in the output array.
- Output ONLY a valid JSON array — no markdown fences, no prose, no comments.
- Each element must follow one of these exact shapes:

  {"issue_number": <int>, "action": "label",   "labels": ["<label>", ...]}
  {"issue_number": <int>, "action": "close",   "comment": "<reason>"}
  {"issue_number": <int>, "action": "comment", "comment": "<question>"}
  {"issue_number": <int>, "action": "none"}

Open issues:
${issues}
PROMPT
	)"

	echo "==> Asking Claude to analyse issues…"
	local actions
	actions=$(run_claude "$prompt")

	# Validate that we got something that looks like JSON
	if ! echo "$actions" | jq -e 'if type == "array" then true else error end' >/dev/null 2>&1; then
		echo "Error: Claude did not return a valid JSON array." >&2
		echo "Output was:" >&2
		echo "$actions" >&2
		return 1
	fi

	local action_count
	action_count=$(echo "$actions" | jq 'length')
	echo "==> Applying ${action_count} action(s)…"

	# Process each action
	echo "$actions" | jq -c '.[]' | while IFS= read -r action; do
		local number action_type
		number=$(echo "$action" | jq -r '.issue_number')
		action_type=$(echo "$action" | jq -r '.action')

		case "$action_type" in
		label)
			local labels_csv
			labels_csv=$(echo "$action" | jq -r '.labels | join(",")')
			echo "  #${number}: label → ${labels_csv}"
			gh issue edit "$number" \
				--repo "$repo" \
				--add-label "$labels_csv"
			;;
		close)
			local comment
			comment=$(echo "$action" | jq -r '.comment // ""')
			echo "  #${number}: close"
			if [[ -n "$comment" ]]; then
				gh issue close "$number" \
					--repo "$repo" \
					--comment "$comment"
			else
				gh issue close "$number" \
					--repo "$repo"
			fi
			;;
		comment)
			local comment
			comment=$(echo "$action" | jq -r '.comment')
			echo "  #${number}: comment"
			gh issue comment "$number" \
				--repo "$repo" \
				--body "$comment"
			;;
		none)
			echo "  #${number}: no action"
			;;
		*)
			echo "  #${number}: unknown action '${action_type}', skipping" >&2
			;;
		esac
	done

	echo "==> Triage complete."
}
