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
SETUP_WORKER_IMAGE="${SIPAG_IMAGE:-ghcr.io/dorky-robot/sipag-worker:latest}"

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

	# --- Pull worker image ---
	echo ""
	echo "Pulling worker image..."
	if ! _setup_docker_image; then
		return 1
	fi

	# --- Create directories ---
	echo ""
	echo "Creating directories..."
	_setup_dirs

	# --- Shell completions ---
	echo ""
	echo "Installing shell completions..."
	_setup_completions

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

# Pull or build the sipag-worker Docker image.
# Pulls from GHCR by default; falls back to local Dockerfile build for custom images.
_setup_docker_image() {
	local image="${SETUP_WORKER_IMAGE}"

	if docker image inspect "$image" >/dev/null 2>&1; then
		_setup_ok "${image} already present"
		return 0
	fi

	# Try to pull from registry first
	printf "  --  Pulling %s... " "$image"
	if docker pull "$image" >/dev/null 2>&1; then
		echo "done"
		_setup_ok "${image} pulled"
		return 0
	fi
	echo "FAILED"

	# Fall back to building from local Dockerfile (useful for custom/local image names)
	local dockerfile_dir
	dockerfile_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

	if [[ -f "${dockerfile_dir}/Dockerfile" ]]; then
		printf "  --  Building %s from Dockerfile... " "$image"
		if docker build -t "$image" "$dockerfile_dir" >/dev/null 2>&1; then
			echo "done"
			_setup_ok "${image} built"
		else
			echo "FAILED"
			_setup_err "Docker build failed — run manually: docker build -t ${image} ${dockerfile_dir}"
			return 1
		fi
	else
		_setup_err "Could not pull ${image} and no Dockerfile found"
		echo "      To use a custom image: SIPAG_IMAGE=my-image sipag setup"
		return 1
	fi
}

# Install shell completions for detected shells.
# Outputs a completion script via 'sipag completions <shell>' and writes it
# to the appropriate location for each detected shell.
_setup_completions() {
	local any_installed=0

	# Bash
	if command -v bash >/dev/null 2>&1; then
		local bash_dir="$HOME/.bash_completion.d"
		mkdir -p "$bash_dir"
		local output
		if output=$(sipag completions bash 2>/dev/null); then
			printf '%s\n' "$output" > "$bash_dir/sipag"
			_setup_ok "Bash completions installed ($bash_dir/sipag)"
			_setup_info "To activate: source $bash_dir/sipag  (add to ~/.bashrc)"
			any_installed=1
		else
			_setup_err "Failed to install bash completions (is sipag-cli installed?)"
		fi
	fi

	# Zsh
	if command -v zsh >/dev/null 2>&1; then
		local zsh_dir="$HOME/.zsh/completions"
		mkdir -p "$zsh_dir"
		local output
		if output=$(sipag completions zsh 2>/dev/null); then
			printf '%s\n' "$output" > "$zsh_dir/_sipag"
			_setup_ok "Zsh completions installed ($zsh_dir/_sipag)"
			_setup_info "Ensure $zsh_dir is in fpath (add to ~/.zshrc: fpath=(~/.zsh/completions \$fpath))"
			any_installed=1
		else
			_setup_err "Failed to install zsh completions (is sipag-cli installed?)"
		fi
	fi

	# Fish
	if command -v fish >/dev/null 2>&1; then
		local fish_dir="$HOME/.config/fish/completions"
		mkdir -p "$fish_dir"
		local output
		if output=$(sipag completions fish 2>/dev/null); then
			printf '%s\n' "$output" > "$fish_dir/sipag.fish"
			_setup_ok "Fish completions installed ($fish_dir/sipag.fish)"
			any_installed=1
		else
			_setup_err "Failed to install fish completions (is sipag-cli installed?)"
		fi
	fi

	if [[ $any_installed -eq 0 ]]; then
		_setup_info "No compatible shells found for completion installation"
		_setup_info "To install manually: sipag completions bash|zsh|fish > <completion-file>"
	fi
}

# Create ~/.sipag/{queue,running,done,failed} directories.
# Idempotent: skips directories that already exist.
_setup_dirs() {
	local sipag_dir="$HOME/.sipag"
	local created=0

	mkdir -p "$sipag_dir"

	for subdir in queue running "done" failed hooks; do
		local dir="${sipag_dir}/${subdir}"
		if [[ ! -d "$dir" ]]; then
			mkdir -p "$dir"
			_setup_ok "Created ~/.sipag/${subdir}/"
			created=$((created + 1))
		fi
	done

	if [[ $created -eq 0 ]]; then
		_setup_ok "$HOME/.sipag/ directories already exist"
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
			_setup_ok "$HOME/.claude/settings.json already configured (skipped)"
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
