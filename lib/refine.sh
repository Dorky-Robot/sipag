#!/usr/bin/env bash
# sipag — Claude-powered issue refinement

# shellcheck source=lib/run.sh
source "$(dirname "${BASH_SOURCE[0]}")/run.sh"

# Refine open GitHub issues using Claude: marks ready, splits, clarifies, or updates.
# Arguments: repo (owner/repo, e.g. "acme/myapp")
# Environment: see lib/run.sh for Claude env vars; GH_TOKEN for GitHub auth
refine_run() {
	local repo="$1"

	# ── Gather ───────────────────────────────────────────────────────────────
	local issues_json
	issues_json=$(gh issue list --repo "$repo" --state open \
		--json number,title,body,labels,comments --limit 50)

	if [[ "$issues_json" == "[]" ]]; then
		echo "[refine] no open issues found"
		return 0
	fi

	# ── Prompt ───────────────────────────────────────────────────────────────
	local prompt
	prompt="You are a software project manager refining GitHub issues.

Analyse these open issues and decide what to do with each one:

${issues_json}

Return a JSON array — one object per issue — with these fields:
  \"issue\"      : integer — the issue number
  \"action\"     : string  — one of: \"ready\", \"split\", \"clarify\", \"update\"
  \"updates\"    : object  — optional; \"title\" and/or \"body\" strings (used for \"update\")
  \"split_into\" : array   — objects with \"title\" and \"body\" strings (used for \"split\")
  \"comment\"    : string  — question to post (used for \"clarify\")
  \"reason\"     : string  — brief explanation

Rules:
- \"ready\"   — issue is well-defined and actionable as-is
- \"split\"   — issue is too large; break into sub-tasks each completable in one PR
- \"clarify\" — requirements are ambiguous; ask a targeted question
- \"update\"  — improve the title and/or body to be clearer and more actionable

Each split task must be independently deliverable in a single PR.
Output valid JSON only. No markdown fences. No explanation text."

	# ── Call Claude ──────────────────────────────────────────────────────────
	local response
	response=$(run_claude "$prompt")

	# ── Apply ────────────────────────────────────────────────────────────────
	local item num action
	while IFS= read -r item; do
		num=$(echo "$item" | jq -r '.issue')
		action=$(echo "$item" | jq -r '.action')

		case "$action" in
		ready)
			gh issue edit "$num" --repo "$repo" --add-label "ready"
			echo "[refine] #${num}: marked ready"
			;;

		split)
			local child_nums=""
			local child child_title child_body child_num
			while IFS= read -r child; do
				child_title=$(echo "$child" | jq -r '.title')
				child_body=$(echo "$child" | jq -r '.body')
				child_num=$(gh issue create --repo "$repo" \
					--title "$child_title" \
					--body "$child_body" \
					--json number --jq '.number')
				child_nums="${child_nums} #${child_num}"
			done < <(echo "$item" | jq -c '.split_into[]')
			local reason
			reason=$(echo "$item" | jq -r '.reason')
			gh issue close "$num" --repo "$repo" \
				--comment "Split into:${child_nums}. ${reason}"
			echo "[refine] #${num}: split into${child_nums}"
			;;

		clarify)
			local comment
			comment=$(echo "$item" | jq -r '.comment')
			gh issue comment "$num" --repo "$repo" --body "$comment"
			echo "[refine] #${num}: clarification requested"
			;;

		update)
			local new_title new_body
			new_title=$(echo "$item" | jq -r '.updates.title // empty')
			new_body=$(echo "$item" | jq -r '.updates.body // empty')
			local update_args=()
			[[ -n "$new_title" ]] && update_args+=("--title" "$new_title")
			[[ -n "$new_body" ]] && update_args+=("--body" "$new_body")
			if [[ ${#update_args[@]} -gt 0 ]]; then
				gh issue edit "$num" --repo "$repo" "${update_args[@]}"
			fi
			echo "[refine] #${num}: updated"
			;;

		*)
			echo "[refine] #${num}: unknown action '${action}', skipping" >&2
			;;
		esac
	done < <(echo "$response" | jq -c '.[]')
}
