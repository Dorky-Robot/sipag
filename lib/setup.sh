#!/usr/bin/env bash
# sipag — setup wizard
#
# Configures Claude Code to auto-allow gh issue/pr/label commands,
# removing permission prompt friction from sipag workflows.

# Permissions to add to Claude Code's allowlist
SETUP_CLAUDE_PERMISSIONS=(
	"Bash(gh issue *)"
	"Bash(gh pr *)"
	"Bash(gh label *)"
)

setup_run() {
	echo "[setup] Configuring sipag..."

	# Check prerequisites
	if ! command -v gh >/dev/null 2>&1; then
		echo "Error: gh CLI required. Install from https://cli.github.com"
		return 1
	fi

	if ! command -v claude >/dev/null 2>&1; then
		echo "Error: claude CLI required. Install from https://claude.ai/code"
		return 1
	fi

	if ! gh auth status >/dev/null 2>&1; then
		echo "Error: gh not authenticated. Run: gh auth login"
		return 1
	fi

	# Configure Claude Code permissions
	if ! _setup_claude_permissions; then
		return 1
	fi

	# Create sipag config dir
	mkdir -p "$HOME/.sipag"
	echo "[setup] Created ~/.sipag/"

	echo ""
	echo "[setup] Done. Configured:"
	echo "  ~/.claude/settings.json — gh issue/pr/label commands are now auto-approved"
	echo "  ~/.sipag/               — sipag config directory"
}

# Merge gh permissions into ~/.claude/settings.json.
# Idempotent: skips entries that are already present.
_setup_claude_permissions() {
	local claude_settings="$HOME/.claude/settings.json"

	mkdir -p "$HOME/.claude"

	# Idempotency check: skip if all permissions already present
	if [[ -f "$claude_settings" ]]; then
		local all_present=1
		for perm in "${SETUP_CLAUDE_PERMISSIONS[@]}"; do
			if ! grep -qF "$perm" "$claude_settings" 2>/dev/null; then
				all_present=0
				break
			fi
		done
		if [[ $all_present -eq 1 ]]; then
			echo "[setup] ~/.claude/settings.json already configured (skipped)"
			return 0
		fi
	fi

	# Merge using jq (preferred) or python3 (fallback)
	if command -v jq >/dev/null 2>&1; then
		_setup_merge_with_jq "$claude_settings"
	elif command -v python3 >/dev/null 2>&1; then
		_setup_merge_with_python "$claude_settings"
	else
		echo "[setup] Warning: jq or python3 required to merge settings automatically."
		echo "[setup] Please add the following to ~/.claude/settings.json manually:"
		echo '  {'
		echo '    "permissions": {'
		echo '      "allow": ['
		for perm in "${SETUP_CLAUDE_PERMISSIONS[@]}"; do
			echo "        \"${perm}\","
		done
		echo '      ]'
		echo '    }'
		echo '  }'
		return 1
	fi
}

_setup_merge_with_jq() {
	local claude_settings="$1"
	local tmp_file
	tmp_file="$(mktemp)"

	local base_json="{}"
	if [[ -f "$claude_settings" ]]; then
		base_json=$(cat "$claude_settings")
	fi

	# Build JSON array of new permissions
	local perms_json
	perms_json=$(printf '"%s"\n' "${SETUP_CLAUDE_PERMISSIONS[@]}" | jq -Rs 'split("\n") | map(select(length > 0))')

	# Merge: append new permissions to existing allow list, then deduplicate
	jq --argjson new_perms "$perms_json" \
		'.permissions.allow = ((.permissions.allow // []) + $new_perms | unique)' \
		<<<"$base_json" >"$tmp_file"

	mv "$tmp_file" "$claude_settings"
	echo "[setup] Updated ~/.claude/settings.json"
}

_setup_merge_with_python() {
	local claude_settings="$1"

	python3 - "$claude_settings" "${SETUP_CLAUDE_PERMISSIONS[@]}" <<'PYEOF'
import json, sys, os

path = sys.argv[1]
new_perms = sys.argv[2:]

data = {}
if os.path.exists(path):
    with open(path) as f:
        data = json.load(f)

data.setdefault("permissions", {}).setdefault("allow", [])

existing = set(data["permissions"]["allow"])
for p in new_perms:
    if p not in existing:
        data["permissions"]["allow"].append(p)

with open(path, "w") as f:
    json.dump(data, f, indent=2)
    f.write("\n")
PYEOF

	echo "[setup] Updated ~/.claude/settings.json"
}
