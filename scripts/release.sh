#!/bin/bash
# Cut a new adsb-decode release.
#
# Usage:
#   bash scripts/release.sh 0.2.11
#   bash scripts/release.sh 0.2.11 "Optional release notes"
#
# What it does:
#   1. Validates the version arg looks like X.Y.Z
#   2. Confirms working tree is clean and on main
#   3. Confirms tests + clippy + fmt pass
#   4. Bumps rust/Cargo.toml workspace.package.version
#   5. Commits the bump
#   6. Creates an annotated tag v<version> pointing at that commit
#   7. Pushes main + the tag, which triggers the CI release job
#
# Why this exists: between v0.2.4 and v0.2.10 the workspace.package.version
# stayed pinned at "0.1.0" because every release was tagged manually without
# updating Cargo.toml. The deployed binary reported `adsb 0.1.0` despite the
# tag being v0.2.10 — confusing for `--version` users and monitoring scripts.
# This script enforces bump-then-tag so the binary version matches the tag.

set -euo pipefail

usage() {
    echo "Usage: $0 <version> [release-notes]"
    echo "Example: $0 0.2.11 \"hotfix for X\""
    exit 1
}

VERSION="${1:-}"
NOTES="${2:-}"

[ -z "$VERSION" ] && usage

# X.Y.Z format check (no leading 'v', no pre-release suffix).
if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "Error: version must look like X.Y.Z (got: $VERSION)"
    exit 1
fi

TAG="v$VERSION"
REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

# Branch + tree state.
BRANCH="$(git rev-parse --abbrev-ref HEAD)"
if [ "$BRANCH" != "main" ]; then
    echo "Error: not on main (currently on '$BRANCH')"
    exit 1
fi
if ! git diff --quiet || ! git diff --cached --quiet; then
    echo "Error: working tree is dirty. Commit or stash first."
    git status -s
    exit 1
fi

# Refuse to clobber an existing tag.
if git rev-parse "$TAG" >/dev/null 2>&1; then
    echo "Error: tag $TAG already exists"
    exit 1
fi

# Pre-flight: tests, clippy, fmt.
echo "=== Running cargo fmt --check ==="
(cd rust && cargo fmt --all -- --check)

echo "=== Running cargo test --workspace ==="
(cd rust && cargo test --workspace)

echo "=== Running cargo clippy --workspace -- -D warnings ==="
(cd rust && cargo clippy --workspace -- -D warnings)

# Update Cargo.toml workspace version. Cross-platform sed (macOS + GNU).
echo "=== Bumping rust/Cargo.toml workspace.package.version to $VERSION ==="
if sed --version >/dev/null 2>&1; then
    sed -i -E 's/^version = ".+"/version = "'"$VERSION"'"/' rust/Cargo.toml
else
    sed -i '' -E 's/^version = ".+"/version = "'"$VERSION"'"/' rust/Cargo.toml
fi

# Verify the bump took.
NEW_VERSION="$(grep '^version = ' rust/Cargo.toml | head -1 | cut -d '"' -f2)"
if [ "$NEW_VERSION" != "$VERSION" ]; then
    echo "Error: Cargo.toml version is '$NEW_VERSION' after bump, expected '$VERSION'"
    git checkout rust/Cargo.toml
    exit 1
fi

# Commit the bump.
git add rust/Cargo.toml
git commit -m "release: $TAG"

# Tag.
TAG_BODY="${NOTES:-Release $TAG}"
git tag -a "$TAG" -m "$TAG_BODY"

# Push branch + tag together. Tag push triggers the CI release job which
# builds adsb-server-x86_64-unknown-linux-gnu-timescaledb.tar.gz and uploads
# it to the GitHub release.
echo "=== Pushing main + $TAG ==="
git push origin main
git push origin "$TAG"

echo
echo "Done. CI is now building the release artifacts."
echo "Once green:"
echo "  ADSB_VPS_HOST=ubuntu@<vps-ip> bash deploy/deploy.sh"
echo "to deploy. Verify with:"
echo "  curl -s https://adsb.blueoctopustechnology.com/api/stats | jq .feed_age_seconds"
