#!/usr/bin/env bash
# scripts/release.sh — Cut a sipag release, optionally upgrade locally.
#
# Usage:
#   scripts/release.sh patch          # 2.2.0 → 2.2.1
#   scripts/release.sh minor          # 2.2.0 → 2.3.0
#   scripts/release.sh major          # 2.2.0 → 3.0.0
#   scripts/release.sh 2.5.0          # explicit version
#   scripts/release.sh minor --local  # skip push, build + install locally
#
# What it does:
#   1. Bumps version in sipag/Cargo.toml
#   2. Runs `cargo check` to update Cargo.lock
#   3. Commits the version bump
#   4. Tags v<version> and pushes (unless --local)
#   5. Optionally waits for the release workflow and runs `brew upgrade`
set -euo pipefail

CARGO_TOML="sipag/Cargo.toml"

# ── Helpers ───────────────────────────────────────────────────────────────────

current_version() {
    grep '^version' "$CARGO_TOML" | head -1 | sed 's/.*"\(.*\)"/\1/'
}

bump_version() {
    local current="$1" bump="$2"
    IFS='.' read -r major minor patch <<< "$current"
    case "$bump" in
        major) echo "$((major + 1)).0.0" ;;
        minor) echo "$major.$((minor + 1)).0" ;;
        patch) echo "$major.$minor.$((patch + 1))" ;;
        *) echo "$bump" ;;  # explicit version
    esac
}

# ── Parse args ────────────────────────────────────────────────────────────────

BUMP="${1:-}"
LOCAL=false

if [[ -z "$BUMP" ]]; then
    echo "Usage: scripts/release.sh <patch|minor|major|X.Y.Z> [--local]"
    exit 1
fi

shift
while [[ $# -gt 0 ]]; do
    case "$1" in
        --local) LOCAL=true ;;
        *) echo "Unknown flag: $1"; exit 1 ;;
    esac
    shift
done

# ── Compute versions ─────────────────────────────────────────────────────────

OLD_VERSION=$(current_version)
NEW_VERSION=$(bump_version "$OLD_VERSION" "$BUMP")
TAG="v${NEW_VERSION}"

echo "Version: $OLD_VERSION → $NEW_VERSION ($TAG)"

# Guard against tagging a version that already exists.
if git tag -l "$TAG" | grep -q "$TAG"; then
    echo "Error: tag $TAG already exists."
    exit 1
fi

# ── Pre-flight: make sure tree is clean (except Cargo.toml we're about to edit)

if ! git diff --quiet -- ':!sipag/Cargo.toml' ':!Cargo.lock'; then
    echo ""
    echo "Warning: you have unstaged changes outside of Cargo.toml."
    echo "These will NOT be included in the release commit."
    echo ""
    git diff --stat -- ':!sipag/Cargo.toml' ':!Cargo.lock'
    echo ""
    read -rp "Continue anyway? [y/N] " yn
    case "$yn" in
        [Yy]*) ;;
        *) echo "Aborted."; exit 1 ;;
    esac
fi

# ── Bump version ──────────────────────────────────────────────────────────────

sed -i '' "s/^version = \"$OLD_VERSION\"/version = \"$NEW_VERSION\"/" "$CARGO_TOML"
echo "Bumped $CARGO_TOML to $NEW_VERSION"

# Update Cargo.lock
cargo check --quiet 2>/dev/null || true

# ── Commit + tag ──────────────────────────────────────────────────────────────

git add "$CARGO_TOML" Cargo.lock
git commit -m "chore: bump version to $NEW_VERSION"
git tag "$TAG"
echo "Created tag $TAG"

if $LOCAL; then
    echo ""
    echo "Local mode — skipping push. Building + installing locally..."
    make install
    echo ""
    sipag version
    echo ""
    echo "Done. When ready to publish: git push origin main $TAG"
    exit 0
fi

# ── Push ──────────────────────────────────────────────────────────────────────

echo "Pushing to origin..."
git push origin main "$TAG"
echo ""
echo "Release workflow triggered: https://github.com/Dorky-Robot/sipag/actions"

# ── Wait for brew (optional) ─────────────────────────────────────────────────

echo ""
read -rp "Wait for release workflow and brew upgrade? [Y/n] " yn
case "$yn" in
    [Nn]*) echo "Done. Run 'brew upgrade sipag' later."; exit 0 ;;
esac

echo "Waiting for release workflow to complete..."
# Poll the release workflow until it finishes (timeout 10 min).
TIMEOUT=600
ELAPSED=0
INTERVAL=15
while [[ $ELAPSED -lt $TIMEOUT ]]; do
    STATUS=$(gh run list --workflow=release.yml --limit=1 --json status --jq '.[0].status' 2>/dev/null || echo "unknown")
    if [[ "$STATUS" == "completed" ]]; then
        CONCLUSION=$(gh run list --workflow=release.yml --limit=1 --json conclusion --jq '.[0].conclusion' 2>/dev/null || echo "unknown")
        if [[ "$CONCLUSION" == "success" ]]; then
            echo "Release workflow succeeded."
            break
        else
            echo "Release workflow finished with: $CONCLUSION"
            exit 1
        fi
    fi
    echo "  Status: $STATUS (${ELAPSED}s / ${TIMEOUT}s)"
    sleep "$INTERVAL"
    ELAPSED=$((ELAPSED + INTERVAL))
done

if [[ $ELAPSED -ge $TIMEOUT ]]; then
    echo "Timed out waiting for release workflow. Check GitHub Actions."
    exit 1
fi

# Give the homebrew tap a moment to propagate.
sleep 5

echo ""
echo "Upgrading via brew..."
brew update --quiet
brew upgrade sipag 2>/dev/null || brew install dorky-robot/tap/sipag
echo ""
sipag version
echo "Done."
