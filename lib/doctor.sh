#!/usr/bin/env bash
# sipag — doctor: diagnose setup problems
#
# Checks every prerequisite and tells you exactly what's missing
# and the exact commands to fix it.

DOCTOR_ERRORS=0
DOCTOR_WARNINGS=0

# Output helpers
_doctor_ok()   { printf "  OK  %s\n" "$*"; }
_doctor_err()  { printf "  ERR %s\n" "$*"; DOCTOR_ERRORS=$((DOCTOR_ERRORS + 1)); }
_doctor_warn() { printf " WARN %s\n" "$*"; DOCTOR_WARNINGS=$((DOCTOR_WARNINGS + 1)); }
_doctor_info() { printf "  --  %s\n" "$*"; }
_doctor_fix()  { printf "      %s\n" "$*"; }

# Check if gh CLI is installed and print its version
_doctor_check_gh() {
	if command -v gh >/dev/null 2>&1; then
		local ver
		ver=$(gh --version 2>/dev/null | head -1 | awk '{print $3}')
		_doctor_ok "gh CLI (${ver})"
	else
		_doctor_err "gh CLI not found"
		printf "\n"
		_doctor_fix "To fix (macOS):   brew install gh"
		_doctor_fix "To fix (Linux):   https://cli.github.com"
		printf "\n"
	fi
}

# Check if claude CLI is installed and print its version
_doctor_check_claude() {
	if command -v claude >/dev/null 2>&1; then
		local ver
		ver=$(claude --version 2>/dev/null | head -1 | awk '{print $NF}')
		_doctor_ok "claude CLI (${ver})"
	else
		_doctor_err "claude CLI not found"
		printf "\n"
		_doctor_fix "To fix:   https://claude.ai/code"
		printf "\n"
	fi
}

# Check if docker CLI is installed and print its version
_doctor_check_docker_installed() {
	if command -v docker >/dev/null 2>&1; then
		local ver
		ver=$(docker --version 2>/dev/null | awk '{print $3}' | tr -d ',')
		_doctor_ok "docker (${ver})"
	else
		_doctor_err "docker not found"
		printf "\n"
		_doctor_fix "To fix (macOS):   brew install --cask docker"
		_doctor_fix "To fix (Linux):   https://docs.docker.com/engine/install/"
		printf "\n"
	fi
}

# Check if jq is installed (optional — warn if missing)
_doctor_check_jq() {
	if command -v jq >/dev/null 2>&1; then
		local ver
		ver=$(jq --version 2>/dev/null | tr -d 'jq-')
		_doctor_ok "jq (${ver})"
	else
		_doctor_warn "jq not found (optional — needed for automatic settings.json merging)"
		printf "\n"
		_doctor_fix "To fix (macOS):   brew install jq"
		_doctor_fix "To fix (Linux):   apt-get install jq  or  yum install jq"
		printf "\n"
	fi
}

# Check if GitHub CLI is authenticated
_doctor_check_gh_auth() {
	if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
		_doctor_ok "GitHub authenticated (gh auth status)"
	else
		_doctor_err "GitHub not authenticated"
		printf "\n"
		_doctor_fix "To fix, run:  gh auth login"
		printf "\n"
	fi
}

# Check if Claude OAuth token exists and is non-empty
_doctor_check_claude_token() {
	local token_file="$HOME/.sipag/token"
	if [[ -f "$token_file" ]] && [[ -s "$token_file" ]]; then
		_doctor_ok "Claude OAuth token (~/.sipag/token)"
	else
		_doctor_err "Claude OAuth token missing (~/.sipag/token)"
		printf "\n"
		_doctor_fix "To fix, run these two commands:"
		printf "\n"
		_doctor_fix "  claude setup-token"
		_doctor_fix "  cp ~/.claude/token ~/.sipag/token"
		printf "\n"
		_doctor_fix "The first command opens your browser to authenticate with Anthropic."
		_doctor_fix "The second copies the token to where sipag workers can use it."
		printf "\n"
		_doctor_fix "Alternative: export ANTHROPIC_API_KEY=sk-ant-... (if you have an API key)"
		printf "\n"
	fi
}

# Note whether ANTHROPIC_API_KEY is set (optional if OAuth token exists)
_doctor_check_api_key() {
	local token_file="$HOME/.sipag/token"
	local has_token=0
	[[ -f "$token_file" ]] && [[ -s "$token_file" ]] && has_token=1

	if [[ -n "${ANTHROPIC_API_KEY:-}" ]]; then
		_doctor_ok "ANTHROPIC_API_KEY set"
	elif [[ $has_token -eq 1 ]]; then
		_doctor_info "ANTHROPIC_API_KEY not set (optional — OAuth token is sufficient)"
	else
		_doctor_info "ANTHROPIC_API_KEY not set (optional — set this or use OAuth token above)"
	fi
}

# Check if Docker daemon is running
_doctor_check_docker_running() {
	if ! command -v docker >/dev/null 2>&1; then
		_doctor_err "Docker daemon not running (docker not installed)"
		return
	fi
	if docker info >/dev/null 2>&1; then
		_doctor_ok "Docker daemon running"
	else
		_doctor_err "Docker daemon not running"
		printf "\n"
		_doctor_fix "To fix:"
		_doctor_fix "  Open Docker Desktop    (macOS)"
		_doctor_fix "  systemctl start docker (Linux)"
		printf "\n"
	fi
}

# Check if sipag-worker:latest image exists
_doctor_check_worker_image() {
	local image="${SIPAG_IMAGE:-sipag-worker:latest}"

	if ! command -v docker >/dev/null 2>&1; then
		_doctor_err "${image} image — docker not installed"
		return
	fi

	if docker image inspect "$image" >/dev/null 2>&1; then
		# Try to get when it was created
		local created
		created=$(docker image inspect "$image" --format '{{.Created}}' 2>/dev/null | cut -dT -f1)
		if [[ -n "$created" ]]; then
			_doctor_ok "${image} image exists (built ${created})"
		else
			_doctor_ok "${image} image exists"
		fi
	else
		_doctor_err "${image} image not found"
		printf "\n"
		_doctor_fix "To fix, run:  sipag setup"
		_doctor_fix "Or manually:  docker build -t ${image} ."
		printf "\n"
	fi
}

# Check if ~/.sipag/ and its subdirectories exist
_doctor_check_sipag_dirs() {
	local sipag_dir="$HOME/.sipag"

	if [[ ! -d "$sipag_dir" ]]; then
		_doctor_err "~/.sipag/ directory missing"
		printf "\n"
		_doctor_fix "To fix, run:  sipag setup"
		printf "\n"
		return
	fi

	_doctor_ok "~/.sipag/ directory exists"

	local missing_dirs=()
	for subdir in queue running done failed; do
		if [[ ! -d "${sipag_dir}/${subdir}" ]]; then
			missing_dirs+=("$subdir")
		fi
	done

	if [[ ${#missing_dirs[@]} -eq 0 ]]; then
		_doctor_ok "Queue directories exist (queue/ running/ done/ failed/)"
	else
		_doctor_err "Queue directories missing: ${missing_dirs[*]}"
		printf "\n"
		_doctor_fix "To fix, run:  sipag setup"
		printf "\n"
	fi
}

# Check if Claude Code permissions include the gh commands
_doctor_check_claude_permissions() {
	local claude_settings="$HOME/.claude/settings.json"

	# SETUP_CLAUDE_PERMISSIONS is defined in setup.sh which is sourced before doctor.sh
	local required_perms=("Bash(gh issue *)" "Bash(gh pr *)" "Bash(gh label *)")

	if [[ ! -f "$claude_settings" ]]; then
		_doctor_err "Claude Code permissions not configured (~/.claude/settings.json missing)"
		printf "\n"
		_doctor_fix "To fix, run:  sipag setup"
		printf "\n"
		return
	fi

	local missing_perms=()
	for perm in "${required_perms[@]}"; do
		if ! grep -qF "$perm" "$claude_settings" 2>/dev/null; then
			missing_perms+=("$perm")
		fi
	done

	if [[ ${#missing_perms[@]} -eq 0 ]]; then
		_doctor_ok "Claude Code permissions configured"
	else
		_doctor_err "Claude Code permissions missing: ${missing_perms[*]}"
		printf "\n"
		_doctor_fix "To fix, run:  sipag setup"
		printf "\n"
	fi
}

doctor_run() {
	DOCTOR_ERRORS=0
	DOCTOR_WARNINGS=0

	echo ""
	echo "=== sipag doctor ==="
	echo ""

	echo "Core tools:"
	_doctor_check_gh
	_doctor_check_claude
	_doctor_check_docker_installed
	_doctor_check_jq
	echo ""

	echo "Authentication:"
	_doctor_check_gh_auth
	_doctor_check_claude_token
	_doctor_check_api_key
	echo ""

	echo "Docker:"
	_doctor_check_docker_running
	_doctor_check_worker_image
	echo ""

	echo "sipag:"
	_doctor_check_sipag_dirs
	_doctor_check_claude_permissions
	echo ""

	# Summary
	if [[ $DOCTOR_ERRORS -eq 0 && $DOCTOR_WARNINGS -eq 0 ]]; then
		echo "All checks passed. Ready to go."
		return 0
	elif [[ $DOCTOR_ERRORS -eq 0 ]]; then
		echo "${DOCTOR_WARNINGS} warning(s). Run 'sipag setup' to address them."
		return 0
	else
		local err_word="errors"
		[[ $DOCTOR_ERRORS -eq 1 ]] && err_word="error"
		echo "${DOCTOR_ERRORS} ${err_word}, ${DOCTOR_WARNINGS} warning(s). Run 'sipag setup' to fix most issues."
		return 1
	fi
}
