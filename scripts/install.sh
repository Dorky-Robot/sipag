#!/usr/bin/env sh
# sipag installer — downloads the correct pre-built binary for your OS/arch.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Dorky-Robot/sipag/main/scripts/install.sh | sh
#
# Options (environment variables):
#   SIPAG_INSTALL_DIR  — where to put the binary    (default: /usr/local/bin)
#   SIPAG_SHARE_DIR    — where to put prompt files   (default: /usr/local/share/sipag)
#   SIPAG_VERSION      — install a specific version  (default: latest)
#
# Works on: Linux (x86_64, aarch64), macOS (x86_64, arm64), Docker containers.
set -eu

REPO="Dorky-Robot/sipag"
INSTALL_DIR="${SIPAG_INSTALL_DIR:-/usr/local/bin}"
SHARE_DIR="${SIPAG_SHARE_DIR:-/usr/local/share/sipag}"

# ── Helpers ──────────────────────────────────────────────────────────────────

log()   { printf '  %s\n' "$*"; }
info()  { printf '\033[1;34m=>\033[0m %s\n' "$*"; }
err()   { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

need() {
  command -v "$1" >/dev/null 2>&1 || err "'$1' is required but not found. Install it and try again."
}

# Run a command with sudo only if needed.
maybe_sudo() {
  if [ -w "$(dirname "$1")" ] 2>/dev/null; then
    "$@"
  elif command -v sudo >/dev/null 2>&1; then
    sudo "$@"
  else
    "$@"  # let it fail with a permission error
  fi
}

# ── Detect platform ─────────────────────────────────────────────────────────

detect_platform() {
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os" in
    Darwin)
      case "$arch" in
        x86_64)       target="x86_64-apple-darwin" ;;
        arm64|aarch64) target="aarch64-apple-darwin" ;;
        *) err "Unsupported macOS architecture: $arch" ;;
      esac
      ;;
    Linux)
      case "$arch" in
        x86_64)       target="x86_64-unknown-linux-gnu" ;;
        aarch64|arm64) target="aarch64-unknown-linux-gnu" ;;
        *) err "Unsupported Linux architecture: $arch" ;;
      esac
      ;;
    *)
      err "Unsupported OS: $os"
      ;;
  esac
}

# ── Resolve version ─────────────────────────────────────────────────────────

resolve_version() {
  if [ -n "${SIPAG_VERSION:-}" ]; then
    version="$SIPAG_VERSION"
    # Ensure it starts with 'v'
    case "$version" in
      v*) ;;
      *)  version="v${version}" ;;
    esac
    return
  fi

  need curl
  info "Fetching latest release..."
  api_url="https://api.github.com/repos/${REPO}/releases/latest"
  release_json="$(curl -fsSL "$api_url")" || err "Failed to fetch release info from GitHub."

  # Extract tag_name without jq (works with grep + sed)
  version="$(printf '%s' "$release_json" | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')"

  [ -n "$version" ] || err "Could not determine latest release version."
}

# ── Download & extract ───────────────────────────────────────────────────────

download() {
  need curl
  need tar

  archive="sipag-${version}-${target}.tar.gz"
  download_url="https://github.com/${REPO}/releases/download/${version}/${archive}"

  tmpdir="$(mktemp -d)"
  # shellcheck disable=SC2064
  trap "rm -rf '$tmpdir'" EXIT

  info "Downloading sipag ${version} for ${target}..."
  curl -fsSL -o "${tmpdir}/${archive}" "$download_url" \
    || err "Download failed. Check that release ${version} exists for ${target}."

  tar -xzf "${tmpdir}/${archive}" -C "$tmpdir"
  extracted="${tmpdir}/sipag-${version}-${target}"

  [ -f "${extracted}/sipag" ] || err "Archive missing sipag binary."
}

# ── Install ──────────────────────────────────────────────────────────────────

install_files() {
  info "Installing sipag to ${INSTALL_DIR}..."

  # Create directories
  maybe_sudo mkdir -p "$INSTALL_DIR"
  maybe_sudo install -m 755 "${extracted}/sipag" "${INSTALL_DIR}/sipag"

  # Install prompt files if present in the release
  if [ -d "${extracted}/lib/prompts" ]; then
    info "Installing prompts to ${SHARE_DIR}..."
    maybe_sudo mkdir -p "${SHARE_DIR}/lib/prompts"
    for f in "${extracted}"/lib/prompts/*.md; do
      [ -f "$f" ] && maybe_sudo install -m 644 "$f" "${SHARE_DIR}/lib/prompts/"
    done
  fi
}

# ── Post-install checks ─────────────────────────────────────────────────────

post_install() {
  echo ""
  info "sipag ${version} installed successfully."
  log "binary:  ${INSTALL_DIR}/sipag"
  [ -d "${SHARE_DIR}/lib/prompts" ] && log "prompts: ${SHARE_DIR}/lib/prompts/"
  echo ""

  # PATH check
  if ! command -v sipag >/dev/null 2>&1; then
    log "Note: ${INSTALL_DIR} is not in your PATH. Add it:"
    log ""
    log "  export PATH=\"${INSTALL_DIR}:\$PATH\""
    log ""
  fi

  # Prerequisite hints
  missing=""
  command -v docker >/dev/null 2>&1 || missing="${missing} docker"
  command -v gh     >/dev/null 2>&1 || missing="${missing} gh"
  command -v claude >/dev/null 2>&1 || missing="${missing} claude"

  if [ -n "$missing" ]; then
    log "Optional prerequisites not found:${missing}"
    log ""
    command -v docker >/dev/null 2>&1 || log "  docker — https://docs.docker.com/get-docker/"
    command -v gh     >/dev/null 2>&1 || log "  gh     — https://cli.github.com"
    command -v claude >/dev/null 2>&1 || log "  claude — npm install -g @anthropic-ai/claude-code"
    echo ""
  fi

  log "Get started:"
  log "  sipag configure    # set up review agents for your project"
  log "  sipag doctor       # check all prerequisites"
}

# ── Main ─────────────────────────────────────────────────────────────────────

detect_platform
resolve_version
download
install_files
post_install
