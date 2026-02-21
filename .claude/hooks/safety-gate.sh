#!/usr/bin/env bash
# sipag — PreToolUse safety gate hook
#
# Receives JSON on stdin from Claude Code, outputs a JSON allow/deny decision.
# Used as a PreToolUse hook to auto-approve safe actions and auto-deny dangerous ones.
#
# Environment:
#   SIPAG_SAFETY_MODE  — strict (default) or balanced; config file can also set this
#   CLAUDE_PROJECT_DIR — project directory (set by Claude Code)
#   ANTHROPIC_API_KEY  — required for balanced mode LLM tiebreaker
#   SIPAG_AUDIT_LOG    — path to audit log file (NDJSON format); unset = no logging
#
# Config file: $CLAUDE_PROJECT_DIR/.claude/hooks/safety-gate.toml (optional)

set -euo pipefail

# Read JSON from stdin
input=$(cat)

tool_name=$(echo "$input" | jq -r '.tool_name // empty')
tool_input=$(echo "$input" | jq -r '.tool_input // empty')

if [[ -z "$tool_name" ]]; then
	exit 0
fi

PROJECT_DIR="${CLAUDE_PROJECT_DIR:-$(pwd)}"
CONFIG_FILE="${PROJECT_DIR}/.claude/hooks/safety-gate.toml"

# Global for audit logging — set before calling allow/deny
AUDIT_SUBJECT=""

# --- Audit logging ---

audit_log() {
	local decision="$1" reason="$2"
	if [[ -n "${SIPAG_AUDIT_LOG:-}" ]]; then
		local timestamp
		timestamp=$(date -u +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u +%Y-%m-%dT%H:%M:%SZ)
		local subject_truncated="${AUDIT_SUBJECT:0:200}"
		jq -cn \
			--arg ts "$timestamp" \
			--arg tool "$tool_name" \
			--arg dec "$decision" \
			--arg reason "$reason" \
			--arg subject "$subject_truncated" \
			'{timestamp: $ts, tool_name: $tool, decision: $dec, reason: $reason, command: $subject}' \
			>>"$SIPAG_AUDIT_LOG" 2>/dev/null || true
	fi
}

# --- Output helpers ---

allow() {
	local reason="${1:-Allowed by safety gate}"
	audit_log "allow" "$reason"
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
	audit_log "deny" "$reason"
	jq -n --arg r "$reason" '{
    hookSpecificOutput: {
      hookEventName: "PreToolUse",
      permissionDecision: "deny",
      permissionDecisionReason: $r
    }
  }'
	exit 0
}

# --- Config file parsing (minimal TOML subset) ---

# Parse all string values from an array field in a TOML section.
# Handles the format:
#   [section]
#   key = [
#     "value1",
#     "value2",
#   ]
# Usage: parse_toml_array <file> <section> <key>
parse_toml_array() {
	local file="$1" section="$2" key="$3"
	[[ -f "$file" ]] || return 0
	awk -v section="[$section]" -v key="$key" '
    /^\[/ { in_section = ($0 == section) }
    in_section && $0 ~ "^" key " = \\[" { in_array = 1; next }
    in_array && /^\]/ { in_array = 0; next }
    in_array {
      gsub(/^[[:space:]]*"/, "")
      gsub(/"[[:space:]]*(,)?[[:space:]]*$/, "")
      if (length($0) > 0) print
    }
  ' "$file"
}

# Parse a single quoted string value from a TOML section.
# Usage: parse_toml_string <file> <section> <key>
parse_toml_string() {
	local file="$1" section="$2" key="$3"
	[[ -f "$file" ]] || return 0
	awk -v section="[$section]" -v key="$key" '
    /^\[/ { in_section = ($0 == section) }
    in_section && $0 ~ "^" key " = " {
      val = $0
      gsub(/^[^=]+= "/, "", val)
      gsub(/"[[:space:]]*$/, "", val)
      print val
      exit
    }
  ' "$file"
}

# --- Load config and set mode ---

_mode_env="${SIPAG_SAFETY_MODE:-}"
EXTRA_DENY_PATTERNS=()
EXTRA_ALLOW_PATTERNS=()
DENY_PATHS=()

if [[ -f "$CONFIG_FILE" ]]; then
	# Mode from config (env var takes precedence)
	if [[ -z "$_mode_env" ]]; then
		_mode_cfg=$(parse_toml_string "$CONFIG_FILE" "mode" "default")
		_mode_env="${_mode_cfg:-}"
	fi

	# Extra deny patterns from config
	while IFS= read -r pattern; do
		[[ -n "$pattern" ]] && EXTRA_DENY_PATTERNS+=("$pattern")
	done < <(parse_toml_array "$CONFIG_FILE" "deny" "patterns")

	# Extra allow patterns from config
	while IFS= read -r pattern; do
		[[ -n "$pattern" ]] && EXTRA_ALLOW_PATTERNS+=("$pattern")
	done < <(parse_toml_array "$CONFIG_FILE" "allow" "patterns")

	# Path deny list from config
	while IFS= read -r dpath; do
		[[ -n "$dpath" ]] && DENY_PATHS+=("$dpath")
	done < <(parse_toml_array "$CONFIG_FILE" "paths" "deny")
fi

SIPAG_SAFETY_MODE="${_mode_env:-strict}"

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

# Check whether a path starts with any entry in the config deny list.
is_path_denied() {
	local path="$1"
	local dpath
	for dpath in "${DENY_PATHS[@]}"; do
		[[ "$path" == "${dpath}"* ]] && return 0
	done
	return 1
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
	# Docker-specific dangerous operations
	'docker run.*--privileged|docker run.*--cap-add'
	'^mount |^umount '
	'^iptables |^ip6tables |^ip route |^ip link '
	'kill -9 |killall '
	'^dd if='
	'^mkfs\.|^mkfs |^fdisk |^parted '
	'>(>)?\s*/proc/|>(>)?\s*/sys/'
	'^(apt|apt-get|yum|dnf|apk) (install|remove|purge|upgrade)'
)

BASH_ALLOW_PATTERNS=(
	'^git (add|commit|status|diff|log|branch|checkout|switch|stash|show|rev-parse|rev-list|ls-files|merge|rebase|cherry-pick|tag|fetch|pull|remote)'
	'^git push( |$)'
	'^(npm|yarn|pnpm) (test|run|exec)'
	'^(cargo|go|python|pytest|make|bundle|rake|mix|gradle|mvn) (test|build|check|lint|format|clippy|install)'
	'^(ls|pwd|which|echo|cat|head|tail|wc|sort|uniq|diff|file|stat|date|env|printenv|true|false)( |$)'
	'^mkdir '
	'^(cp|mv) '
	'^(npm|yarn|pnpm) install'
	'^pip install'
	'^chmod [0-7]{3} '
	'^node |^python |^ruby '
	# Docker (non-privileged)
	'^docker (ps|images|logs|inspect|info|run --rm|rm|stop|pull|build)'
	# Development tools
	'^bats '
	'^make (test|check|lint|fmt|dev|build|clean|install|all)( |$)'
	'^gh (issue|pr|repo|release|workflow|run|auth|api)'
	# sipag commands (bare or full path)
	'sipag (work|start|merge|setup|doctor|ps|logs|status|version|help)'
)

check_bash_deny() {
	local cmd="$1"
	local pattern
	for pattern in "${BASH_DENY_PATTERNS[@]}" "${EXTRA_DENY_PATTERNS[@]}"; do
		if echo "$cmd" | grep -qE "$pattern"; then
			return 0
		fi
	done
	return 1
}

check_bash_allow() {
	local cmd="$1"
	local pattern
	for pattern in "${BASH_ALLOW_PATTERNS[@]}" "${EXTRA_ALLOW_PATTERNS[@]}"; do
		if echo "$cmd" | grep -qE "$pattern"; then
			return 0
		fi
	done
	return 1
}

# --- LLM tiebreaker (balanced mode) ---

llm_evaluate() {
	local subject="$1"

	if [[ -z "${ANTHROPIC_API_KEY:-}" ]]; then
		deny "Ambiguous command denied (no API key for LLM evaluation): $subject"
	fi

	local response
	response=$(curl -s --max-time 10 \
		-H "x-api-key: ${ANTHROPIC_API_KEY}" \
		-H "anthropic-version: 2023-06-01" \
		-H "content-type: application/json" \
		-d "$(jq -n \
			--arg cmd "$subject" \
			--arg project "$PROJECT_DIR" \
			'{
        model: "claude-haiku-4-5-20251001",
        max_tokens: 100,
        system: "You are a security evaluator for an unattended coding agent. The agent is working on a GitHub issue in a cloned repo. Evaluate if this shell command is safe to run. Respond with only ALLOW or DENY followed by a one-sentence reason.",
        messages: [{role: "user", content: ("Command: " + $cmd + "\nProject directory: " + $project)}]
      }')" \
		"https://api.anthropic.com/v1/messages" 2>/dev/null) || {
		deny "Ambiguous command denied (LLM evaluation failed): $subject"
	}

	local text
	text=$(echo "$response" | jq -r '.content[0].text // empty' 2>/dev/null)

	if [[ -z "$text" ]]; then
		deny "Ambiguous command denied (empty LLM response): $subject"
	fi

	if [[ "$text" == ALLOW* ]]; then
		allow "LLM approved: $text"
	else
		deny "LLM denied: $text"
	fi
}

# --- Main evaluation ---

case "$tool_name" in
Read | Glob | Grep | Task | TaskOutput | TaskStop | WebSearch | WebFetch | AskUserQuestion | Skill | NotebookEdit | EnterPlanMode | ExitPlanMode | EnterWorktree | TaskCreate | TaskGet | TaskUpdate | TaskList)
	AUDIT_SUBJECT="$tool_name"
	allow "Read-only tool: $tool_name"
	;;

Edit | Write)
	file_path=$(echo "$tool_input" | jq -r '.file_path // empty')
	AUDIT_SUBJECT="${file_path}"
	if [[ -z "$file_path" ]]; then
		deny "No file_path in $tool_name input"
	fi
	if is_path_denied "$file_path"; then
		deny "$tool_name targets denied path: $file_path"
	fi
	if is_within_project "$file_path"; then
		allow "$tool_name within project directory"
	else
		deny "$tool_name targets path outside project: $file_path"
	fi
	;;

Bash)
	cmd=$(echo "$tool_input" | jq -r '.command // empty')
	AUDIT_SUBJECT="${cmd}"
	if [[ -z "$cmd" ]]; then
		deny "Empty bash command"
	fi

	# rm is allowed only within the project directory.
	if echo "$cmd" | grep -qE '^rm '; then
		# Extract file paths from rm command (skip flags like -f, -r, -rf).
		local rm_ok=true
		local arg
		for arg in $cmd; do
			[[ "$arg" == "rm" ]] && continue
			[[ "$arg" == -* ]] && continue
			if ! is_within_project "$arg"; then
				rm_ok=false
				break
			fi
		done
		if $rm_ok; then
			allow "rm within project directory"
		else
			deny "rm targets path outside project: $cmd"
		fi
	fi

	if check_bash_allow "$cmd"; then
		allow "Command matches allow pattern"
	fi

	if check_bash_deny "$cmd"; then
		deny "Command matches deny pattern: $cmd"
	fi

	if [[ "$SIPAG_SAFETY_MODE" == "balanced" ]]; then
		llm_evaluate "$cmd"
	else
		deny "Ambiguous command denied in strict mode: $cmd"
	fi
	;;

*)
	AUDIT_SUBJECT="$tool_input"
	if [[ "$SIPAG_SAFETY_MODE" == "balanced" ]]; then
		llm_evaluate "Tool: $tool_name, Input: $tool_input"
	else
		deny "Unknown tool denied in strict mode: $tool_name"
	fi
	;;
esac
