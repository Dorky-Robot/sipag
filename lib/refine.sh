#!/usr/bin/env bash
# sipag — GitHub issue refinement via Claude
#
# Fetches open issues from a repository, prompts Claude to classify and
# break them down, then applies the resulting actions via the gh CLI.

# Analyse open issues in <repo> and apply refinement actions.
# Arguments: repo (owner/repo format)
# Requires: gh, claude (via run_claude from lib/run.sh), jq
refine_run() {
	local repo="$1"

	if [[ -z "$repo" ]]; then
		echo "Error: repo argument required (owner/repo)" >&2
		return 1
	fi

	echo "==> Fetching open issues from ${repo}..."

	local issues
	issues="$(gh issue list \
		--repo "$repo" \
		--state open \
		--json number,title,body,labels,comments \
		--limit 50)"

	local count
	count="$(printf '%s' "$issues" | jq 'length')"

	if [[ "$count" -eq 0 ]]; then
		echo "No open issues in ${repo}"
		return 0
	fi

	echo "==> Analysing ${count} issue(s) with Claude..."

	local prompt
	prompt="$(cat <<PROMPT
You are a product manager helping to refine GitHub issues for the repository ${repo}.

Analyse the following open issues and return a JSON array of actions.
For each issue choose exactly ONE action:

  ready   — the issue is well-defined and ready to implement as-is
  split   — the issue is too large; break it into smaller, independently
             deliverable tasks (each completable in a single PR)
  clarify — requirements are ambiguous; ask specific clarifying questions
  update  — the title or body can be made clearer and more actionable

Rules:
- Only mark an issue as "ready" when it has clear acceptance criteria and
  a well-bounded scope.
- For "split": provide 2–5 sub-issues; each must be independently
  mergeable in one PR.
- For "clarify": ask 1–3 specific, targeted questions.
- For "update": supply an improved title and/or body.
- Output valid JSON only — no markdown fences, no prose, no explanation.

JSON schema (return an array, one element per issue):
[
  { "action": "ready",   "number": <n> },
  { "action": "split",   "number": <n>,
    "sub_issues": [ { "title": "<t>", "body": "<b>" }, ... ] },
  { "action": "clarify", "number": <n>, "comment": "<questions>" },
  { "action": "update",  "number": <n>,
    "title": "<new title or omit if unchanged>",
    "body":  "<new body or omit if unchanged>" }
]

Issues (JSON):
${issues}
PROMPT
)"

	local response
	if ! response="$(run_claude "$prompt")"; then
		echo "Error: Claude invocation failed" >&2
		return 1
	fi

	# Validate that the response is a JSON array before iterating
	if ! printf '%s' "$response" | jq -e 'type == "array"' >/dev/null 2>&1; then
		echo "Error: Claude returned non-array JSON:" >&2
		printf '%s\n' "$response" >&2
		return 1
	fi

	echo "==> Applying actions..."

	local action action_type number
	while IFS= read -r action; do
		action_type="$(printf '%s' "$action" | jq -r '.action')"
		number="$(printf '%s' "$action" | jq -r '.number')"

		case "$action_type" in
		ready)
			echo "  #${number}: ready — adding label"
			gh issue edit "$number" --repo "$repo" --add-label "ready"
			;;

		split)
			echo "  #${number}: split — creating sub-issues"
			local -a created=()
			local sub_issue sub_title sub_body new_number

			while IFS= read -r sub_issue; do
				sub_title="$(printf '%s' "$sub_issue" | jq -r '.title')"
				sub_body="$(printf '%s' "$sub_issue" | jq -r '.body')"
				new_number="$(gh issue create \
					--repo "$repo" \
					--title "$sub_title" \
					--body "$sub_body" \
					--json number \
					--jq '.number')"
				created+=("#${new_number}")
				echo "    created #${new_number}: ${sub_title}"
			done < <(printf '%s' "$action" | jq -c '.sub_issues[]')

			local close_comment
			close_comment="Split into: $(
				IFS=', '
				printf '%s' "${created[*]}"
			)"
			gh issue comment "$number" --repo "$repo" --body "$close_comment"
			gh issue close "$number" --repo "$repo"
			echo "    closed #${number}"
			;;

		clarify)
			echo "  #${number}: clarify — posting comment"
			local comment
			comment="$(printf '%s' "$action" | jq -r '.comment')"
			gh issue comment "$number" --repo "$repo" --body "$comment"
			;;

		update)
			echo "  #${number}: update — editing issue"
			local -a edit_args=("$number" --repo "$repo")
			local new_title new_body
			new_title="$(printf '%s' "$action" | jq -r '.title // empty')"
			new_body="$(printf '%s' "$action" | jq -r '.body // empty')"
			[[ -n "$new_title" ]] && edit_args+=(--title "$new_title")
			[[ -n "$new_body" ]] && edit_args+=(--body "$new_body")
			gh issue edit "${edit_args[@]}"
			;;

		*)
			echo "  #${number}: unknown action '${action_type}' — skipping" >&2
			;;
		esac
	done < <(printf '%s' "$response" | jq -c '.[]')

	echo "==> Done"
}
