#!/usr/bin/env sh
# sipag installer — downloads the correct pre-built binary for your OS/arch.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Dorky-Robot/sipag/main/scripts/install.sh | sh
#
# Options (environment variables):
#   SIPAG_INSTALL_DIR  — where to put the binary    (default: /usr/local/bin)
#   SIPAG_VERSION      — install a specific version  (default: latest)
#
# Works on: Linux (x86_64, aarch64), macOS (x86_64, arm64), Docker containers.
set -eu

REPO="Dorky-Robot/sipag"
INSTALL_DIR="${SIPAG_INSTALL_DIR:-/usr/local/bin}"

# ── Helpers ──────────────────────────────────────────────────────────────────

log()   { printf '  %s\n' "$*"; }
info()  { printf '\033[1;34m=>\033[0m %s\n' "$*"; }
err()   { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

need() {
  command -v "$1" >/dev/null 2>&1 || err "'$1' is required but not found. Install it and try again."
}

# Run a command with sudo only when the target directory is not writable.
# Usage: maybe_sudo <target_path> <command> [args...]
maybe_sudo() {
  _target="$1"; shift
  if [ -w "$(dirname "$_target")" ] 2>/dev/null; then
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
    # Validate version format (vX.Y.Z)
    case "$version" in
      v[0-9]*.[0-9]*.[0-9]*) ;;
      *) err "Invalid version format: $version (expected vX.Y.Z)" ;;
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
  maybe_sudo "$INSTALL_DIR" mkdir -p "$INSTALL_DIR"
  maybe_sudo "${INSTALL_DIR}/sipag" install -m 755 "${extracted}/sipag" "${INSTALL_DIR}/sipag"
}

# ── Post-install checks ─────────────────────────────────────────────────────

post_install() {
  echo ""
  info "sipag ${version} installed successfully."
  log "binary:  ${INSTALL_DIR}/sipag"
  echo ""

  # PATH check
  if ! command -v sipag >/dev/null 2>&1; then
    log "Note: ${INSTALL_DIR} is not in your PATH. Add it:"
    log ""
    log "  export PATH=\"${INSTALL_DIR}:\$PATH\""
    log ""
  fi

  log "Get started:"
  log "  sipag version           # confirm install"
  log "  sipag project add ...   # register a project"
  log "  sipag tui               # open the kanban board"
}

# ── Main ─────────────────────────────────────────────────────────────────────

detect_platform
resolve_version
download
install_files
post_install
