#!/usr/bin/env bash
# sipag — Claude-powered issue triage command

# Triage all open issues in a GitHub repository using Claude.
# Fetches issues, asks Claude to classify them, then applies labels/comments/closes.
# Arguments: repo (owner/name)
# Requires: gh, jq, run_claude (from lib/run.sh)
triage_run() {
	local repo="$1"

	# ── Gather ────────────────────────────────────────────────────────────────
	local issues_json
	issues_json=$(gh issue list --repo "$repo" --state open \
		--json number,title,body,labels,comments,createdAt --limit 100)

	if [[ "$issues_json" == "[]" ]]; then
		echo "[triage] no open issues found in ${repo}"
		return 0
	fi

	# ── Prompt ────────────────────────────────────────────────────────────────
	local prompt
	prompt=$(cat <<EOF
You are an expert GitHub issue triager. Given the following open issues from the repository "${repo}", output a JSON array of triage actions — one object per issue.

Each object must have these fields:
  "issue":    the issue number (integer)
  "action":   one of "label", "close", "comment", "none"
  "labels":   array of strings — category labels from [bug, enhancement, refactor, docs, test] plus priority P0-P3
  "priority": one of "P0", "P1", "P2", "P3"  (P0=critical outage, P1=high, P2=medium, P3=nice-to-have)
  "comment":  string — only populate if action is "comment" and clarification is genuinely needed
  "reason":   string — one-line explanation of your decision

Rules:
- Close issues that are exact duplicates or already resolved by an existing merged PR.
- Every issue gets a priority label (P0–P3) plus one category label.
- Add a comment only when essential information is missing and the issue cannot be acted on otherwise.
- Output valid JSON only — no markdown fences, no prose before or after the array.

Issues JSON:
${issues_json}
EOF
)

	local response
	response=$(run_claude "$prompt")

	# ── Apply ─────────────────────────────────────────────────────────────────
	local length
	length=$(echo "$response" | jq 'length')

	local i
	for (( i = 0; i < length; i++ )); do
		local entry num action priority comment reason
		entry=$(echo "$response" | jq ".[$i]")
		num=$(echo "$entry" | jq -r '.issue')
		action=$(echo "$entry" | jq -r '.action')
		priority=$(echo "$entry" | jq -r '.priority')
		comment=$(echo "$entry" | jq -r '.comment // ""')
		reason=$(echo "$entry" | jq -r '.reason // ""')

		case "$action" in
		label)
			# Apply category labels
			local label_count lbl_i lbl
			label_count=$(echo "$entry" | jq '.labels | length')
			for (( lbl_i = 0; lbl_i < label_count; lbl_i++ )); do
				lbl=$(echo "$entry" | jq -r ".labels[$lbl_i]")
				gh issue edit "$num" --repo "$repo" --add-label "$lbl" 2>/dev/null || true
			done
			# Apply priority label
			gh issue edit "$num" --repo "$repo" --add-label "$priority" 2>/dev/null || true
			echo "[triage] #${num}: labelled (priority=${priority}, reason=${reason})"
			;;
		close)
			gh issue close "$num" --repo "$repo" --comment "$reason" 2>/dev/null || true
			echo "[triage] #${num}: closed (reason=${reason})"
			;;
		comment)
			gh issue comment "$num" --repo "$repo" --body "$comment" 2>/dev/null || true
			echo "[triage] #${num}: commented (reason=${reason})"
			;;
		none)
			echo "[triage] #${num}: no action needed (reason=${reason})"
			;;
		*)
			echo "[triage] #${num}: unknown action '${action}' — skipping" >&2
			;;
		esac
	done
}
