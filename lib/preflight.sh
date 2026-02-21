#!/usr/bin/env bash
# sipag — preflight checks: validate prerequisites before running commands

SIPAG_DIR="${SIPAG_DIR:-$HOME/.sipag}"

# Check that Claude authentication is available.
# OAuth token (~/.sipag/token) takes priority; ANTHROPIC_API_KEY is a fallback.
preflight_auth() {
    if [[ -s "${SIPAG_DIR}/token" ]]; then
        return 0
    fi
    if [[ -n "${ANTHROPIC_API_KEY:-}" ]]; then
        echo "Note: Using ANTHROPIC_API_KEY. For OAuth instead, run:"
        echo "  claude setup-token"
        echo "  cp ~/.claude/token ${SIPAG_DIR}/token"
        return 0
    fi
    echo "Error: No Claude authentication found."
    echo ""
    echo "  To fix, run these two commands:"
    echo ""
    echo "    claude setup-token"
    echo "    cp ~/.claude/token ${SIPAG_DIR}/token"
    echo ""
    echo "  The first command opens your browser to authenticate with Anthropic."
    echo "  The second copies the token to where sipag workers can use it."
    echo ""
    echo "  Alternative: export ANTHROPIC_API_KEY=sk-ant-... (if you have an API key)"
    return 1
}

# Check that Docker daemon is running.
preflight_docker_running() {
    if docker info &>/dev/null; then
        return 0
    fi
    echo "Error: Docker is not running."
    echo ""
    echo "  To fix:"
    echo ""
    echo "    Open Docker Desktop    (macOS)"
    echo "    systemctl start docker (Linux)"
    return 1
}

# Check that the required Docker image exists.
# Usage: preflight_docker_image [image]
preflight_docker_image() {
    local image="${1:-sipag-worker:latest}"
    if docker image inspect "$image" &>/dev/null; then
        return 0
    fi
    echo "Error: Docker image '${image}' not found."
    echo ""
    echo "  To fix, run:"
    echo ""
    echo "    sipag setup"
    echo ""
    echo "  Or build manually:"
    echo ""
    echo "    docker build -t ${image} ."
    return 1
}

# Check that the GitHub CLI is authenticated.
preflight_gh_auth() {
    if gh auth status &>/dev/null; then
        return 0
    fi
    echo "Error: GitHub CLI is not authenticated."
    echo ""
    echo "  To fix, run:"
    echo ""
    echo "    gh auth login"
    return 1
}

# Check that a GitHub repository is accessible.
# Usage: preflight_repo <owner/repo>
preflight_repo() {
    local repo="$1"
    if gh repo view "$repo" &>/dev/null; then
        return 0
    fi
    echo "Error: Cannot access repository '${repo}'."
    echo ""
    echo "  Make sure the repository exists and you have access:"
    echo ""
    echo "    gh repo view ${repo}"
    return 1
}
