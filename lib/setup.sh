#!/usr/bin/env bash
# sipag — setup wizard
#
# Takes you from installed to fully operational in one command.

# Permissions to add to Claude Code's allowlist
SETUP_CLAUDE_PERMISSIONS=(
	"Bash(gh issue *)"
	"Bash(gh pr *)"
	"Bash(gh label *)"
)

# Docker image name for workers
SETUP_WORKER_IMAGE="${SIPAG_IMAGE:-sipag-worker:latest}"

# Output helpers
_setup_ok()   { printf "  OK  %s\n" "$*"; }
_setup_err()  { printf "  ERR %s\n" "$*"; }
_setup_info() { printf "  --  %s\n" "$*"; }

setup_run() {
	local failed=0

	echo ""
	echo "=== sipag setup ==="
	echo ""

	# --- Prerequisite checks ---
	echo "Checking prerequisites..."

	if command -v gh >/dev/null 2>&1; then
		_setup_ok "gh CLI installed"
	else
		_setup_err "gh CLI required — install from https://cli.github.com"
		failed=1
	fi

	if command -v claude >/dev/null 2>&1; then
		_setup_ok "claude CLI installed"
	else
		_setup_err "claude CLI required — install from https://claude.ai/code"
		failed=1
	fi

	if gh auth status >/dev/null 2>&1; then
		_setup_ok "GitHub authenticated"
	else
		_setup_err "gh not authenticated — run: gh auth login"
		failed=1
	fi

	if command -v docker >/dev/null 2>&1; then
		_setup_ok "Docker installed"
		if docker info >/dev/null 2>&1; then
			_setup_ok "Docker running"
		else
			_setup_err "Docker not running — please start Docker Desktop"
			failed=1
		fi
	else
		_setup_err "Docker not installed — install from https://docs.docker.com/get-docker/"
		failed=1
	fi

	if [[ $failed -eq 1 ]]; then
		echo ""
		echo "Fix the errors above and re-run: sipag setup"
		return 1
	fi

	# --- Configure Claude Code permissions ---
	echo ""
	echo "Configuring Claude Code permissions..."
	if ! _setup_claude_permissions; then
		return 1
	fi

	# --- Authentication ---
	echo ""
	echo "Setting up authentication..."
	_setup_auth

	# --- Build worker image ---
	echo ""
	echo "Building worker image..."
	if ! _setup_docker_image; then
		return 1
	fi

	# --- Create directories ---
	echo ""
	echo "Creating directories..."
	_setup_dirs

	echo ""
	echo "=== Setup complete ==="
	echo ""
	echo "Next: open claude and type: sipag start <owner/repo>"
	echo ""
}

# Set up Claude OAuth token (primary auth method).
# Falls back to ANTHROPIC_API_KEY if OAuth setup fails.
_setup_auth() {
	local token_file="$HOME/.sipag/token"
	local claude_token_file="$HOME/.claude/token"

	mkdir -p "$HOME/.sipag"

	if [[ -f "$token_file" ]] && [[ -s "$token_file" ]]; then
		_setup_ok "Claude OAuth token configured (~/.sipag/token)"
	else
		_setup_err "Claude OAuth token missing (~/.sipag/token)"
		echo "      Running: claude setup-token"
		if claude setup-token 2>&1; then
			if [[ -f "$claude_token_file" ]] && [[ -s "$claude_token_file" ]]; then
				cp "$claude_token_file" "$token_file"
				echo "      Copied token to ~/.sipag/token"
				_setup_ok "Claude OAuth token configured (primary auth)"
			else
				_setup_err "Could not find token after claude setup-token (expected ~/.claude/token)"
				echo "      Try manually: claude setup-token && cp ~/.claude/token ~/.sipag/token"
			fi
		else
			_setup_err "claude setup-token failed"
			echo "      Try manually: claude setup-token && cp ~/.claude/token ~/.sipag/token"
		fi
	fi

	# ANTHROPIC_API_KEY is optional fallback only
	if [[ -n "${ANTHROPIC_API_KEY:-}" ]]; then
		_setup_ok "ANTHROPIC_API_KEY set (optional fallback)"
	else
		_setup_info "ANTHROPIC_API_KEY not set (optional fallback — OAuth token is sufficient)"
	fi
}

# Build or verify the sipag-worker Docker image.
_setup_docker_image() {
	local image="${SETUP_WORKER_IMAGE}"

	if docker image inspect "$image" >/dev/null 2>&1; then
		_setup_ok "${image} exists"
		return 0
	fi

	# Try to find the Dockerfile relative to this script
	local dockerfile_dir
	dockerfile_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

	if [[ -f "${dockerfile_dir}/Dockerfile" ]]; then
		printf "  --  Building %s... " "$image"
		if docker build -t "$image" "$dockerfile_dir" >/dev/null 2>&1; then
			echo "done"
			_setup_ok "${image} built"
		else
			echo "FAILED"
			_setup_err "Docker build failed — run manually: docker build -t ${image} ${dockerfile_dir}"
			return 1
		fi
	else
		_setup_err "${image} not found and no Dockerfile available to build from"
		echo "      Run: docker build -t ${image} /path/to/sipag"
		return 1
	fi
}

# Create ~/.sipag/{queue,running,done,failed} directories.
# Idempotent: skips directories that already exist.
_setup_dirs() {
	local sipag_dir="$HOME/.sipag"
	local created=0

	mkdir -p "$sipag_dir"

	for subdir in queue running done failed hooks; do
		local dir="${sipag_dir}/${subdir}"
		if [[ ! -d "$dir" ]]; then
			mkdir -p "$dir"
			_setup_ok "Created ~/.sipag/${subdir}/"
			created=$((created + 1))
		fi
	done

	if [[ $created -eq 0 ]]; then
		_setup_ok "~/.sipag/ directories already exist"
	fi
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
			_setup_ok "~/.claude/settings.json already configured (skipped)"
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
	_setup_ok "Updated ~/.claude/settings.json"
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

	_setup_ok "Updated ~/.claude/settings.json"
}
