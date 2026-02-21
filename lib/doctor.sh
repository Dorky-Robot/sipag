#!/usr/bin/env bash
# sipag — doctor: diagnose setup problems and print exact fix commands

SIPAG_DIR="${SIPAG_DIR:-$HOME/.sipag}"

# Module-level counter init (reset by doctor_run on each invocation)
_doctor_errors=0
_doctor_warnings=0

# Output helpers
_doctor_ok()   { printf "  OK  %s\n" "$*"; }
_doctor_err()  { printf "  ERR %s\n" "$*"; _doctor_errors=$(( _doctor_errors + 1 )); }
_doctor_warn() { printf " WARN %s\n" "$*"; _doctor_warnings=$(( _doctor_warnings + 1 )); }
_doctor_info() { printf "  --  %s\n" "$*"; }

# Run all prerequisite checks and print diagnostics.
# Exit code: 0 = all OK, 1 = errors found.
doctor_run() {
	_doctor_errors=0
	_doctor_warnings=0

	echo ""
	echo "=== sipag doctor ==="
	echo ""

	# --- Core tools ---
	echo "Core tools:"

	local gh_version
	if command -v gh &>/dev/null; then
		gh_version=$(gh --version 2>/dev/null | head -1 | awk '{print $3}')
		_doctor_ok "gh CLI (${gh_version})"
	else
		_doctor_err "gh not found"
		printf "\n      To fix:\n"
		printf "        brew install gh        (macOS)\n"
		printf "        https://cli.github.com (other)\n\n"
	fi

	local claude_version
	if command -v claude &>/dev/null; then
		claude_version=$(claude --version 2>/dev/null | head -1 | awk '{print $1}')
		_doctor_ok "claude CLI (${claude_version})"
	else
		_doctor_err "claude not found"
		printf "\n      To fix:\n"
		printf "        https://claude.ai/code\n\n"
	fi

	local docker_version
	if command -v docker &>/dev/null; then
		docker_version=$(docker --version 2>/dev/null | awk '{print $3}' | tr -d ',')
		_doctor_ok "docker (${docker_version})"
	else
		_doctor_err "docker not found"
		printf "\n      To fix (macOS):   brew install --cask docker\n"
		printf "      To fix (Linux):   https://docs.docker.com/engine/install/\n\n"
	fi

	local jq_version
	if command -v jq &>/dev/null; then
		jq_version=$(jq --version 2>/dev/null | sed 's/^jq-//')
		_doctor_ok "jq (${jq_version})"
	else
		_doctor_warn "jq not found (optional, recommended for sipag status)"
		printf "\n      To fix:\n"
		printf "        brew install jq   (macOS)\n"
		printf "        apt install jq    (Debian/Ubuntu)\n\n"
	fi

	# --- Authentication ---
	echo ""
	echo "Authentication:"

	if gh auth status &>/dev/null; then
		_doctor_ok "GitHub authenticated (gh auth status)"
	else
		_doctor_err "GitHub not authenticated"
		printf "\n      To fix, run:  gh auth login\n\n"
	fi

	if [[ -s "${SIPAG_DIR}/token" ]]; then
		_doctor_ok "Claude OAuth token (~/.sipag/token)"
	else
		_doctor_err "Claude OAuth token missing (~/.sipag/token)"
		printf "\n      To fix, run these two commands:\n\n"
		printf "        claude setup-token\n"
		printf "        cp ~/.claude/token ~/.sipag/token\n\n"
		printf "      The first command opens your browser to authenticate with Anthropic.\n"
		printf "      The second copies the token to where sipag workers can use it.\n\n"
		printf "      Alternative: export ANTHROPIC_API_KEY=sk-ant-... (if you have an API key)\n\n"
	fi

	if [[ -n "${ANTHROPIC_API_KEY:-}" ]]; then
		_doctor_info "ANTHROPIC_API_KEY set (optional — OAuth token is sufficient)"
	else
		_doctor_info "ANTHROPIC_API_KEY not set (optional — OAuth token is sufficient)"
	fi

	# --- Docker ---
	echo ""
	echo "Docker:"

	if command -v docker &>/dev/null; then
		if docker info &>/dev/null; then
			_doctor_ok "Docker daemon running"
		else
			_doctor_err "Docker daemon not running"
			printf "\n      To fix:\n"
			printf "        Open Docker Desktop    (macOS)\n"
			printf "        systemctl start docker (Linux)\n\n"
		fi

		local image="${SIPAG_IMAGE:-ghcr.io/dorky-robot/sipag-worker:latest}"
		if docker image inspect "$image" &>/dev/null; then
			_doctor_ok "${image} image exists"
		else
			_doctor_err "${image} image not found"
			printf "\n      To fix, run:  sipag setup\n"
			printf "      Or manually:  docker build -t %s .\n\n" "$image"
		fi
	else
		_doctor_info "Docker checks skipped (docker not installed)"
	fi

	# --- sipag ---
	echo ""
	echo "sipag:"

	if [[ -d "${SIPAG_DIR}" ]]; then
		_doctor_ok "~/.sipag/ directory exists"
	else
		_doctor_err "~/.sipag/ directory missing"
		printf "\n      To fix, run:  sipag setup\n\n"
	fi

	local subdir missing_dirs
	missing_dirs=()
	for subdir in queue running done failed; do
		if [[ ! -d "${SIPAG_DIR}/${subdir}" ]]; then
			missing_dirs+=("$subdir")
		fi
	done
	if [[ ${#missing_dirs[@]} -eq 0 ]]; then
		_doctor_ok "Queue directories exist"
	else
		_doctor_err "Queue directories missing: ${missing_dirs[*]}"
		printf "\n      To fix, run:  sipag setup\n\n"
	fi

	local claude_settings="$HOME/.claude/settings.json"
	local required_perms=("Bash(gh issue *)" "Bash(gh pr *)" "Bash(gh label *)")
	local perm missing_perms
	missing_perms=()
	for perm in "${required_perms[@]}"; do
		if ! grep -qF "$perm" "$claude_settings" 2>/dev/null; then
			missing_perms+=("$perm")
		fi
	done
	if [[ ${#missing_perms[@]} -eq 0 ]]; then
		_doctor_ok "Claude Code permissions configured"
	else
		_doctor_err "Claude Code permissions missing"
		printf "\n      Missing permissions:\n"
		for perm in "${missing_perms[@]}"; do
			printf "        %s\n" "$perm"
		done
		printf "      To fix, run:  sipag setup\n\n"
	fi

	# --- Summary ---
	echo ""
	if [[ $_doctor_errors -eq 0 && $_doctor_warnings -eq 0 ]]; then
		echo "All checks passed. Ready to go."
	elif [[ $_doctor_errors -eq 0 ]]; then
		echo "${_doctor_warnings} warning(s). Run 'sipag setup' to fix most issues."
	else
		local summary="${_doctor_errors} error(s)"
		if [[ $_doctor_warnings -gt 0 ]]; then
			summary="${summary}, ${_doctor_warnings} warning(s)"
		fi
		echo "${summary}. Run 'sipag setup' to fix most issues."
	fi

	if [[ $_doctor_errors -gt 0 ]]; then
		return 1
	fi
	return 0
}
