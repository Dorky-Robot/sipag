#!/usr/bin/env bash
# sipag — PreToolUse safety gate hook (deny-list-only)
#
# Receives JSON on stdin from Claude Code, outputs a JSON allow/deny decision.
# Used as a PreToolUse hook to auto-approve safe actions and auto-deny dangerous ones.
#
# Architecture: deny known-dangerous operations, allow everything else.
# There is no allow-list, no LLM tiebreaker, and no mode system.
#
# Environment:
#   CLAUDE_PROJECT_DIR — project directory (set by Claude Code)
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

# --- Load config ---

EXTRA_DENY_PATTERNS=()
DENY_PATHS=()

if [[ -f "$CONFIG_FILE" ]]; then
	# Extra deny patterns from config
	while IFS= read -r pattern; do
		[[ -n "$pattern" ]] && EXTRA_DENY_PATTERNS+=("$pattern")
	done < <(parse_toml_array "$CONFIG_FILE" "deny" "patterns")

	# Path deny list from config
	while IFS= read -r dpath; do
		[[ -n "$dpath" ]] && DENY_PATHS+=("$dpath")
	done < <(parse_toml_array "$CONFIG_FILE" "paths" "deny")
fi

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

# Check whether a path is within any safe directory (project, ~/.sipag, ~/.claude).
is_within_safe_dir() {
	local path="$1"
	if is_within_project "$path"; then
		return 0
	fi
	if [[ "$path" != /* ]]; then
		path="${PROJECT_DIR}/${path}"
	fi
	local parent
	parent=$(cd "$(dirname "$path")" 2>/dev/null && pwd) || return 1
	local resolved
	resolved="${parent}/$(basename "$path")"
	[[ "$resolved" == "${HOME}/.sipag/"* || "$resolved" == "${HOME}/.sipag" ]] && return 0
	[[ "$resolved" == "${HOME}/.claude/"* || "$resolved" == "${HOME}/.claude" ]] && return 0
	return 1
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
	'^sudo |^doas '
	'^rm -rf [/~]|^rm -rf \*'
	'^git push.* --force|^git push.* -f( |$)'
	'^git reset --hard|^git clean -f'
	'>(>)?\s*/proc/|>(>)?\s*/sys/'
	'chmod 777|chown '
	'^curl.*-X (POST|PUT|DELETE|PATCH)|^curl.*--data|^curl.*-d |^wget '
	'^ssh |^scp |^rsync '
	'^(npm|pip|gem) install -g|^npm i -g'
	'^eval |^exec '
	'\|\s*(ba)?sh\s*$'
	# Docker-specific dangerous operations
	'docker run.*--privileged|docker run.*--cap-add'
	'^mount |^umount '
	'^iptables |^ip6tables |^ip route |^ip link '
	'^kill -9 |^killall '
	'^dd if='
	'^mkfs\.|^mkfs |^fdisk |^parted '
	'^(apt|apt-get|yum|dnf|apk) (install|remove|purge|upgrade)'
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

# Check whether an rm command only targets paths within safe directories.
check_rm_safe() {
	local cmd="$1"
	local rm_ok=true
	local arg
	for arg in $cmd; do
		[[ "$arg" == "rm" ]] && continue
		[[ "$arg" == -* ]] && continue
		if ! is_within_safe_dir "$arg"; then
			rm_ok=false
			break
		fi
	done
	if $rm_ok; then
		allow "rm within safe directory"
	else
		deny "rm targets path outside safe directories: $cmd"
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
	if is_within_safe_dir "$file_path"; then
		allow "$tool_name within safe directory"
	else
		deny "$tool_name targets path outside safe directories: $file_path"
	fi
	;;

Bash)
	cmd=$(echo "$tool_input" | jq -r '.command // empty')
	AUDIT_SUBJECT="${cmd}"
	if [[ -z "$cmd" ]]; then
		deny "Empty bash command"
	fi

	# rm is allowed within the project directory, ~/.sipag, and ~/.claude.
	if echo "$cmd" | grep -qE '^rm '; then
		check_rm_safe "$cmd"
	fi

	# Deny check is the only gate — if it matches, deny; otherwise allow.
	if check_bash_deny "$cmd"; then
		deny "Command matches deny pattern: $cmd"
	fi

	allow "Command not on deny list"
	;;

*)
	AUDIT_SUBJECT="$tool_input"
	allow "Tool allowed: $tool_name"
	;;
esac
