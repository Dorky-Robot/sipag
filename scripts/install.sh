#!/usr/bin/env bash
# sipag installer — downloads the correct pre-built binary for your OS/arch.
# Usage: curl -fsSL https://raw.githubusercontent.com/Dorky-Robot/sipag/main/scripts/install.sh | bash
set -euo pipefail

REPO="Dorky-Robot/sipag"
INSTALL_DIR="${SIPAG_INSTALL_DIR:-/usr/local/bin}"
SHARE_DIR="${SIPAG_SHARE_DIR:-/usr/local/share/sipag}"

# ── Detect OS ─────────────────────────────────────────────────────────────────

os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
  Darwin)
    case "$arch" in
      x86_64)  target="x86_64-apple-darwin" ;;
      arm64)   target="aarch64-apple-darwin" ;;
      *)       echo "Unsupported macOS architecture: $arch" >&2; exit 1 ;;
    esac
    ;;
  Linux)
    case "$arch" in
      x86_64)  target="x86_64-unknown-linux-gnu" ;;
      aarch64|arm64) target="aarch64-unknown-linux-gnu" ;;
      *)       echo "Unsupported Linux architecture: $arch" >&2; exit 1 ;;
    esac
    ;;
  *)
    echo "Unsupported OS: $os" >&2
    exit 1
    ;;
esac

# ── Resolve latest release ────────────────────────────────────────────────────

echo "Fetching latest sipag release..."
api_url="https://api.github.com/repos/${REPO}/releases/latest"
release_json="$(curl -fsSL "$api_url")"

# Extract version tag (works without jq)
version="$(echo "$release_json" | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')"

if [ -z "$version" ]; then
  echo "Could not determine latest release version." >&2
  exit 1
fi

archive="sipag-${version}-${target}.tar.gz"
download_url="https://github.com/${REPO}/releases/download/${version}/${archive}"

# ── Download ──────────────────────────────────────────────────────────────────

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

echo "Downloading sipag ${version} for ${target}..."
curl -fsSL -o "${tmpdir}/${archive}" "$download_url"

# ── Extract ───────────────────────────────────────────────────────────────────

tar -xzf "${tmpdir}/${archive}" -C "$tmpdir"
extracted_dir="${tmpdir}/sipag-${version}-${target}"

# ── Install bash scripts to share prefix ─────────────────────────────────────
#
# Layout: ${SHARE_DIR}/
#   lib/               — shell-out scripts (setup, doctor, start, merge, refresh-docs)
#   lib/container/     — container entrypoint scripts (embedded in Rust via include_str)
#   lib/prompts/       — prompt templates

_install() {
  if [ -w "$(dirname "$2")" ]; then
    install "$@"
  else
    sudo install "$@"
  fi
}

_mkdir() {
  if [ -w "$(dirname "$1")" ] 2>/dev/null || [ -d "$1" ]; then
    mkdir -p "$1"
  else
    sudo mkdir -p "$1"
  fi
}

echo "Installing bash scripts to ${SHARE_DIR}..."
_mkdir "${SHARE_DIR}/lib/container"
_mkdir "${SHARE_DIR}/lib/prompts"

_install -m 644 "${extracted_dir}"/lib/*.sh         "${SHARE_DIR}/lib/"
_install -m 644 "${extracted_dir}"/lib/container/*.sh "${SHARE_DIR}/lib/container/"
_install -m 644 "${extracted_dir}"/lib/prompts/*.md  "${SHARE_DIR}/lib/prompts/"

# ── Install binary ───────────────────────────────────────────────────────────

echo "Installing sipag binary to ${INSTALL_DIR}..."
_mkdir "${INSTALL_DIR}"
_install -m 755 "${extracted_dir}/sipag" "${INSTALL_DIR}/sipag"

# ── PATH reminder ─────────────────────────────────────────────────────────────

if ! command -v sipag >/dev/null 2>&1; then
  echo ""
  echo "Note: ${INSTALL_DIR} is not in your PATH."
  echo "Add it by running:"
  echo ""
  echo "  echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> ~/.bashrc  # bash"
  echo "  echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> ~/.zshrc   # zsh"
  echo ""
fi

# ── Done ──────────────────────────────────────────────────────────────────────

echo ""
echo "sipag ${version} installed."
echo "  scripts: ${SHARE_DIR}"
echo "  command: ${INSTALL_DIR}/sipag"
echo ""
echo "Prerequisites (install separately if not present):"
echo "  jq        — brew install jq / apt install jq"
echo "  gh        — brew install gh / https://cli.github.com"
echo "  Docker    — https://www.docker.com/products/docker-desktop"
echo "  Claude Code — npm install -g @anthropic-ai/claude-code"
echo ""
echo "Next step:"
echo "  sipag setup"
