#!/usr/bin/env bash
# sipag installer — downloads the correct pre-built binary for your OS/arch.
# Usage: curl -fsSL https://raw.githubusercontent.com/Dorky-Robot/sipag/main/scripts/install.sh | bash
set -euo pipefail

REPO="Dorky-Robot/sipag"
INSTALL_DIR="${SIPAG_INSTALL_DIR:-/usr/local/bin}"

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

# ── Install binary ────────────────────────────────────────────────────────────

if [ -w "$INSTALL_DIR" ]; then
  install -m 755 "${extracted_dir}/sipag" "${INSTALL_DIR}/sipag"
else
  echo "Installing to ${INSTALL_DIR} (requires sudo)..."
  sudo install -m 755 "${extracted_dir}/sipag" "${INSTALL_DIR}/sipag"
fi

# ── Install bash scripts ──────────────────────────────────────────────────────

SIPAG_DIR="${HOME}/.sipag"
mkdir -p "${SIPAG_DIR}/bin" "${SIPAG_DIR}/lib"

install -m 755 "${extracted_dir}/bin/sipag" "${SIPAG_DIR}/bin/sipag"
install -m 644 "${extracted_dir}"/lib/*.sh "${SIPAG_DIR}/lib/"

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
echo "sipag ${version} installed to ${INSTALL_DIR}/sipag"
echo ""
echo "Next step:"
echo "  sipag setup"
