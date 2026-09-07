#!/usr/bin/env bash
# Publish a new release: build macOS binaries, create the GitHub release,
# and update the Homebrew formula to point at it.
#
# Usage:
#   1. Set the new version in Cargo.toml and commit it.
#   2. Run: ./scripts/release.sh
#
# Requires: cargo with both macOS targets, gh (logged in), a clean tree.
set -euo pipefail

cd "$(dirname "$0")/.."

REPO="kwansing14/blackjack-tui"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
TAG="v$VERSION"
TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
DIST=dist

if [[ -z "$VERSION" ]]; then
  echo "error: could not read version from Cargo.toml" >&2
  exit 1
fi

if [[ -n "$(git status --porcelain)" ]]; then
  echo "error: working tree is not clean; commit the version bump first" >&2
  exit 1
fi

if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
  echo "error: tag $TAG already exists" >&2
  exit 1
fi

echo "==> Releasing $TAG"

echo "==> Building"
rm -rf "$DIST" && mkdir -p "$DIST"
for target in "${TARGETS[@]}"; do
  cargo build --release --locked --target "$target"
  tar -C "target/$target/release" -czf "$DIST/blackjack-$target.tar.gz" blackjack
done

echo "==> Tagging and pushing"
git tag "$TAG"
git push origin main "$TAG"

echo "==> Creating GitHub release"
gh release create "$TAG" "$DIST"/blackjack-*.tar.gz \
  --repo "$REPO" --title "$TAG" --generate-notes

echo "==> Updating Formula/blackjack.rb"
sha_arm=$(shasum -a 256 "$DIST/blackjack-aarch64-apple-darwin.tar.gz" | cut -d' ' -f1)
sha_intel=$(shasum -a 256 "$DIST/blackjack-x86_64-apple-darwin.tar.gz" | cut -d' ' -f1)
base="https://github.com/$REPO/releases/download/$TAG"

cat > Formula/blackjack.rb <<RUBY
class Blackjack < Formula
  desc "Two-player blackjack in the terminal, over the internet"
  homepage "https://github.com/$REPO"
  version "$VERSION"

  on_arm do
    url "$base/blackjack-aarch64-apple-darwin.tar.gz"
    sha256 "$sha_arm"
  end
  on_intel do
    url "$base/blackjack-x86_64-apple-darwin.tar.gz"
    sha256 "$sha_intel"
  end

  def install
    bin.install "blackjack"
  end

  test do
    assert_match "usage", shell_output("#{bin}/blackjack 2>&1", 2)
  end
end
RUBY

git add Formula/blackjack.rb
git commit -m "Formula: $TAG"
git push origin main

rm -rf "$DIST"
echo "==> Done. Users update with: brew update && brew upgrade blackjack"
