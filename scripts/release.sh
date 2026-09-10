#!/usr/bin/env bash
# Publish a release: build macOS binaries, tag, create the GitHub release, and point the
# Homebrew formula at it. Safe to re-run: steps that already happened are skipped, so a
# run that died halfway (network, gh auth) is finished by running the same command again.
#
# Usage:
#   1. Set the new version in Cargo.toml and commit it.
#   2. npm run release        (same as ./scripts/release.sh)
#
# Requires: cargo with both macOS targets, gh (logged in), a clean tree.
set -euo pipefail

cd "$(dirname "$0")/.."

REPO="kwansing14/blackjack-tui" # only for the download URLs; git and gh use the origin remote
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

echo "==> Releasing $TAG"

echo "==> Building"
rm -rf "$DIST" && mkdir -p "$DIST"
for target in "${TARGETS[@]}"; do
  cargo build --release --locked --target "$target"
  tar -C "target/$target/release" -czf "$DIST/blackjack-$target.tar.gz" blackjack
done

echo "==> Tagging and pushing"
if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
  if [[ "$(git rev-parse "$TAG^{commit}")" != "$(git rev-parse HEAD)" ]]; then
    echo "error: tag $TAG exists but points at another commit; bump the version in Cargo.toml" >&2
    exit 1
  fi
  echo "tag $TAG already exists, reusing it"
else
  git tag "$TAG"
fi
git push origin main "$TAG"

echo "==> GitHub release"
if gh release view "$TAG" >/dev/null 2>&1; then
  echo "release $TAG already exists, reusing it"
else
  # no assets on this line: gh deletes the whole release if any asset upload fails
  gh release create "$TAG" --generate-notes
fi

# Upload one file at a time and trust the release's asset list, not gh's exit status.
# The proxy can drop the reply after GitHub has stored the file; gh then retries, gets
# "ReleaseAsset.name already exists", and reports failure for an upload that worked.
uploaded_size() {
  gh release view "$TAG" --json assets --jq ".assets[] | select(.name == \"$1\") | .size"
}
for f in "$DIST"/*; do
  name=$(basename "$f")
  size=$(stat -f%z "$f")
  for attempt in 1 2 3; do
    gh release upload "$TAG" "$f" --clobber || true
    if [[ "$(uploaded_size "$name")" == "$size" ]]; then
      break
    fi
    if [[ $attempt == 3 ]]; then
      echo "error: $name did not upload after 3 attempts; run again" >&2
      exit 1
    fi
    echo "retrying $name"
  done
done

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

if git diff --quiet Formula/blackjack.rb; then
  echo "formula already points at $TAG"
else
  git add Formula/blackjack.rb
  git commit -m "Formula: $TAG"
  git push origin main
fi

rm -rf "$DIST"
echo "==> Done. Users update with: brew update && brew upgrade blackjack"
