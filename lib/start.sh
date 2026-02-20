#!/usr/bin/env bash
# sipag — interactive agile session launcher
#
# Provides: start_run <mode> <owner/repo>
# Modes: triage, refinement, review

_start_prompt_triage() {
	local repo="$1"
	cat <<EOF
You are a technical project manager helping triage GitHub issues for ${repo}.

Go through the open issues and:
- Label them appropriately (bug, enhancement, question, documentation, etc.)
- Identify duplicates and close them with a reference to the original
- Add priority labels (high, medium, low) where the project uses them
- Close invalid or won't-fix issues with a polite explanatory comment
- Summarize what you've done when finished

Use: gh issue list --repo ${repo} --state open --limit 50
EOF
}

_start_prompt_refinement() {
	local repo="$1"
	cat <<EOF
You are a technical product manager helping refine GitHub issues for ${repo}.

Go through the open issues and add the detail needed for a developer to start work:
- Add clear acceptance criteria (what done looks like)
- Add implementation notes or pointers to relevant code
- Break down large issues into smaller, actionable sub-tasks
- Clarify ambiguous requirements by reading related code and leaving comments

Use: gh issue list --repo ${repo} --state open --limit 50
EOF
}

_start_prompt_review() {
	local repo="$1"
	cat <<EOF
You are a senior engineer helping review pull requests for ${repo}.

Go through the open pull requests and:
- Review code changes for correctness, quality, and style
- Check that tests are adequate and passing
- Leave constructive, specific review comments
- Approve PRs that meet the bar and are ready to merge
- Request changes where improvements are needed

Use: gh pr list --repo ${repo} --state open
EOF
}

start_run() {
	local mode="$1"
	local repo="$2"

	local prompt
	case "$mode" in
	triage)
		prompt="$(_start_prompt_triage "$repo")"
		;;
	refinement)
		prompt="$(_start_prompt_refinement "$repo")"
		;;
	review)
		prompt="$(_start_prompt_review "$repo")"
		;;
	*)
		printf 'Error: unknown mode "%s"\n' "$mode" >&2
		printf 'Valid modes: triage, refinement, review\n' >&2
		return 1
		;;
	esac

	printf '==> sipag start %s %s\n' "$mode" "$repo"
	claude --print --dangerously-skip-permissions -p "$prompt"
}
