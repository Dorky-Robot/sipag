#!/usr/bin/env bash
# sipag — Claude-powered PR review command

# shellcheck source=lib/run.sh
source "$(dirname "${BASH_SOURCE[0]}")/run.sh"

# Review all open PRs in a repository using Claude.
# Arguments: repo — GitHub owner/repo (e.g. Dorky-Robot/sipag)
review_run() {
	local repo="$1"

	# Gather open PR numbers — portable loop (no mapfile for macOS compat)
	local prs=()
	while IFS= read -r num; do
		[[ -n "$num" ]] && prs+=("$num")
	done < <(gh pr list --repo "$repo" --state open --json number -q '.[].number' | sort -n)

	if [[ ${#prs[@]} -eq 0 ]]; then
		echo "[review] No open PRs."
		return
	fi

	local pr_num
	for pr_num in "${prs[@]}"; do
		local pr_json diff prompt response verdict body

		pr_json=$(gh pr view "$pr_num" --repo "$repo" --json title,body,files,comments)
		diff=$(gh pr diff "$pr_num" --repo "$repo")

		prompt="You are reviewing a GitHub pull request. Analyze the PR metadata and diff carefully.

PR metadata (JSON):
${pr_json}

Diff:
${diff}

Output a JSON object with exactly this structure (no markdown fences):
{
  \"verdict\": \"approve\" | \"request_changes\" | \"comment\",
  \"summary\": \"one-line summary of the PR\",
  \"comments\": [
    {
      \"path\": \"file path\",
      \"line\": <line number>,
      \"body\": \"review comment\"
    }
  ],
  \"body\": \"overall review comment\"
}

Rules:
- Approve if the code is correct, tests pass, and no issues
- Request changes for bugs, security issues, or missing tests
- Be specific about what needs to change
- Output valid JSON only, no markdown fences"

		response=$(run_claude "$prompt")

		verdict=$(printf '%s' "$response" | jq -r '.verdict // "comment"' 2>/dev/null) || verdict="comment"
		body=$(printf '%s' "$response" | jq -r '.body // ""' 2>/dev/null) || body=""

		case "$verdict" in
			approve)
				gh pr review "$pr_num" --repo "$repo" --approve --body "$body"
				;;
			request_changes)
				gh pr review "$pr_num" --repo "$repo" --request-changes --body "$body"
				;;
			*)
				gh pr review "$pr_num" --repo "$repo" --comment --body "$body"
				verdict="comment"
				;;
		esac

		echo "[review] PR #${pr_num}: ${verdict}"
	done
}
