#!/usr/bin/env bash
# sipag — PreToolUse safety gate hook
#
# Receives JSON on stdin from Claude Code, outputs a JSON allow/deny decision.
# Used as a PreToolUse hook to auto-approve safe actions and auto-deny dangerous ones.
#
# Environment:
#   SIPAG_SAFETY_MODE  — strict (default) or balanced
#   CLAUDE_PROJECT_DIR — project directory (set by Claude Code)
#   ANTHROPIC_API_KEY  — required for balanced mode LLM tiebreaker

set -euo pipefail

# Read JSON from stdin
input=$(cat)

tool_name=$(echo "$input" | jq -r '.tool_name // empty')
tool_input=$(echo "$input" | jq -r '.tool_input // empty')

if [[ -z "$tool_name" ]]; then
	exit 0
fi

SIPAG_SAFETY_MODE="${SIPAG_SAFETY_MODE:-strict}"
PROJECT_DIR="${CLAUDE_PROJECT_DIR:-$(pwd)}"

# --- Output helpers ---

allow() {
	local reason="${1:-Allowed by safety gate}"
	jq -n --arg r "$reason" '{
    hookSpecificOutput: {
      hookEventName: "PreToolUse",
      permissionDecision: "allow",
      permissionDecisionReason: $r
    }
  }'
	exit 0
}

deny() {
	local reason="${1:-Denied by safety gate}"
	jq -n --arg r "$reason" '{
    hookSpecificOutput: {
      hookEventName: "PreToolUse",
      permissionDecision: "deny",
      permissionDecisionReason: $r
    }
  }'
	exit 0
}

# --- Path validation ---

is_within_project() {
	local path="$1"
	if [[ "$path" != /* ]]; then
		path="${PROJECT_DIR}/${path}"
	fi
	# Normalize by resolving the parent directory to handle .. traversal.
	# If the parent doesn't exist, deny conservatively.
	local parent
	parent=$(cd "$(dirname "$path")" 2>/dev/null && pwd) || return 1
	local resolved
	resolved="${parent}/$(basename "$path")"
	[[ "$resolved" == "${PROJECT_DIR}/"* || "$resolved" == "${PROJECT_DIR}" ]]
}

# --- Bash command evaluation ---

BASH_DENY_PATTERNS=(
	'sudo|doas'
	'rm -rf [/~]|rm -rf \*'
	'git push.* --force( |$)|git push.* -f( |$)'
	'git reset --hard|git clean -f'
	'>(>)?\s*/etc/|>(>)?\s*/usr/|>(>)?\s*~/'
	'chmod 777|chown '
	'curl.*-X (POST|PUT|DELETE|PATCH)|curl.*--data|curl.*-d |wget '
	'ssh |scp |rsync '
	'(npm|pip|gem) install -g|npm i -g'
	'eval |exec '
	'\|.*sh$|\|.*bash$'
	'/etc/|/usr/|\.ssh/|\.gnupg/|\.aws/'
)

BASH_ALLOW_PATTERNS=(
	'^git (add|commit|status|diff|log|branch|checkout|switch|stash|show|rev-parse|rev-list|ls-files|merge|rebase|cherry-pick|tag|fetch|pull|remote)'
	'^git push( |$)'
	'^(npm|yarn|pnpm) (test|run|exec)'
	'^(cargo|go|python|pytest|make|bundle|rake|mix|gradle|mvn) (test|build|check|lint|format|clippy)'
	'^(ls|pwd|which|echo|cat|head|tail|wc|sort|uniq|diff|file|stat|date|env|printenv|true|false)( |$)'
	'^mkdir '
	'^(cp|mv) '
	'^(npm|yarn|pnpm) install'
	'^pip install'
	'^chmod [0-7]{3} '
	'^node |^python |^ruby '
)

check_bash_deny() {
	local cmd="$1"
	for pattern in "${BASH_DENY_PATTERNS[@]}"; do
		if echo "$cmd" | grep -qE "$pattern"; then
			return 0
		fi
	done
	return 1
}

check_bash_allow() {
	local cmd="$1"
	for pattern in "${BASH_ALLOW_PATTERNS[@]}"; do
		if echo "$cmd" | grep -qE "$pattern"; then
			return 0
		fi
	done
	return 1
}

# --- LLM tiebreaker (balanced mode) ---

llm_evaluate() {
	local cmd="$1"

	if [[ -z "${ANTHROPIC_API_KEY:-}" ]]; then
		deny "Ambiguous command denied (no API key for LLM evaluation): $cmd"
	fi

	local response
	response=$(curl -s --max-time 10 \
		-H "x-api-key: ${ANTHROPIC_API_KEY}" \
		-H "anthropic-version: 2023-06-01" \
		-H "content-type: application/json" \
		-d "$(jq -n \
			--arg cmd "$cmd" \
			--arg project "$PROJECT_DIR" \
			'{
        model: "claude-haiku-4-5-20251001",
        max_tokens: 100,
        system: "You are a security evaluator for an unattended coding agent. The agent is working on a GitHub issue in a cloned repo. Evaluate if this shell command is safe to run. Respond with only ALLOW or DENY followed by a one-sentence reason.",
        messages: [{role: "user", content: ("Command: " + $cmd + "\nProject directory: " + $project)}]
      }')" \
		"https://api.anthropic.com/v1/messages" 2>/dev/null) || {
		deny "Ambiguous command denied (LLM evaluation failed): $cmd"
	}

	local text
	text=$(echo "$response" | jq -r '.content[0].text // empty' 2>/dev/null)

	if [[ -z "$text" ]]; then
		deny "Ambiguous command denied (empty LLM response): $cmd"
	fi

	if [[ "$text" == ALLOW* ]]; then
		allow "LLM approved: $text"
	else
		deny "LLM denied: $text"
	fi
}

# --- Main evaluation ---

case "$tool_name" in
Read | Glob | Grep | Task | WebSearch | WebFetch)
	allow "Read-only tool: $tool_name"
	;;

Edit | Write)
	file_path=$(echo "$tool_input" | jq -r '.file_path // empty')
	if [[ -z "$file_path" ]]; then
		deny "No file_path in $tool_name input"
	fi
	if is_within_project "$file_path"; then
		allow "$tool_name within project directory"
	else
		deny "$tool_name targets path outside project: $file_path"
	fi
	;;

Bash)
	cmd=$(echo "$tool_input" | jq -r '.command // empty')
	if [[ -z "$cmd" ]]; then
		deny "Empty bash command"
	fi

	if check_bash_deny "$cmd"; then
		deny "Command matches deny pattern: $cmd"
	fi

	if check_bash_allow "$cmd"; then
		allow "Command matches allow pattern"
	fi

	if [[ "$SIPAG_SAFETY_MODE" == "balanced" ]]; then
		llm_evaluate "$cmd"
	else
		deny "Ambiguous command denied in strict mode: $cmd"
	fi
	;;

*)
	if [[ "$SIPAG_SAFETY_MODE" == "balanced" ]]; then
		llm_evaluate "Tool: $tool_name, Input: $tool_input"
	else
		deny "Unknown tool denied in strict mode: $tool_name"
	fi
	;;
esac
