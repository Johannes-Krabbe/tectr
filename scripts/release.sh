#!/usr/bin/env bash
# Builds tectr for release on this Mac, and with `publish`, ships it through
# GitHub Releases to the Homebrew tap. Everything runs locally, so releases cost
# no CI minutes.
#
#   scripts/release.sh build            build and package the current version
#   scripts/release.sh publish 0.2.0    bump, build, tag, push, release, update the cask
set -euo pipefail

REPO="Johannes-Krabbe/tectr"
TAP="johannes-krabbe/tap"
RELEASE_PORT=7767

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
dist="$root/target/release-dist"

fail() {
  echo "release: $*" >&2
  exit 1
}

current_version() {
  sed -n -E 's/^version = "([^"]+)"$/\1/p' Cargo.toml | head -n 1
}

tarball_for() {
  echo "$dist/tectr_$1_darwin_arm64.tar.gz"
}

build() {
  local version
  version="$(current_version)"
  [[ "$(uname -sm)" == "Darwin arm64" ]] || fail "builds only on an Apple Silicon Mac"

  # Tauri compiles the Liquid Glass icon with actool, which only Xcode 26 or later
  # has. Without it Tauri logs an error and ships the flat .icns, so use Xcode even
  # when xcode-select points at the Command Line Tools.
  if ! actool --version >/dev/null 2>&1; then
    [[ -d /Applications/Xcode.app ]] || fail "needs Xcode 26 or later for the app icon"
    export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
  fi

  # Picks the release port and data folders in protocol::Channel. Without it the
  # build would be a dev build.
  export TECTR_CHANNEL=release
  pnpm install --frozen-lockfile

  # Tauri runs actool with stdin closed. actool then starts its ibtoold helper
  # without a stdin, which fails with "Bad file descriptor", and since ibtoold
  # stays running, every later actool run fails the same way until it's killed.
  # The shim gives actool a stdin.
  local shim="$root/target/actool-shim"
  mkdir -p "$shim"
  printf '#!/bin/sh\nexec /usr/bin/actool "$@" </dev/null\n' >"$shim/actool"
  chmod +x "$shim/actool"
  PATH="$shim:$PATH" pnpm --dir apps/dsk tauri build
  cargo build --release --locked -p daemon -p cli

  # A dev build would quietly use the dev port and data, so check before shipping.
  target/release/tectr-daemon --help | grep -q "default: $RELEASE_PORT" ||
    fail "the daemon isn't a release build"
  [[ -f target/release/bundle/macos/tectr.app/Contents/Resources/Assets.car ]] ||
    fail "the app has no Liquid Glass icon"

  rm -rf "$dist"
  mkdir -p "$dist/stage"
  cp -R target/release/bundle/macos/tectr.app "$dist/stage/"
  cp target/release/tectr target/release/tectr-daemon "$dist/stage/"
  tar -czf "$(tarball_for "$version")" -C "$dist/stage" tectr.app tectr tectr-daemon
  echo "Built $(tarball_for "$version")"
}

publish() {
  local version="${1:-}"
  [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "give a version like 0.2.0"
  local tag="v$version"

  [[ "$(git branch --show-current)" == "main" ]] || fail "release from main"
  [[ -z "$(git status --porcelain)" ]] || fail "commit or stash your changes first"
  git remote get-url origin >/dev/null 2>&1 || fail "there's no origin remote"
  ! git rev-parse -q --verify "refs/tags/$tag" >/dev/null || fail "$tag already exists"
  gh auth status >/dev/null 2>&1 || fail "log in with gh auth login first"
  local tap_dir
  tap_dir="$(brew --repo "$TAP")"
  [[ -d "$tap_dir/.git" ]] || fail "tap $TAP isn't tapped. Run brew tap $TAP"
  [[ -z "$(git -C "$tap_dir" status --porcelain)" ]] || fail "the tap at $tap_dir has uncommitted changes"
  git -C "$tap_dir" pull --ff-only --quiet

  sed -i '' -E "s/^version = \"[^\"]+\"$/version = \"$version\"/" Cargo.toml
  sed -i '' -E "s/^  \"version\": \"[^\"]+\",$/  \"version\": \"$version\",/" \
    apps/dsk/package.json apps/dsk/src-tauri/tauri.conf.json
  cargo update --workspace --quiet

  # Build before committing anything, so a failed build leaves only the version
  # bump to undo.
  build
  local tarball sha
  tarball="$(tarball_for "$version")"
  sha="$(shasum -a 256 "$tarball" | cut -d ' ' -f 1)"

  git add Cargo.toml Cargo.lock apps/dsk/package.json apps/dsk/src-tauri/tauri.conf.json
  git diff --cached --quiet || git commit --quiet -m "Release $tag"
  git tag -a "$tag" -m "$tag"
  git push --quiet origin main "$tag"
  gh release create "$tag" "$tarball" --repo "$REPO" --title "$tag" --generate-notes

  sed -e "s/@VERSION@/$version/" -e "s/@SHA256@/$sha/" packaging/homebrew/tectr.rb \
    >"$tap_dir/Casks/tectr.rb"
  git -C "$tap_dir" add Casks/tectr.rb
  git -C "$tap_dir" commit --quiet -m "tectr $version"
  git -C "$tap_dir" push --quiet
  echo "Released $tag. Install or upgrade with: brew upgrade --cask tectr || brew install --cask $TAP/tectr"
}

case "${1:-}" in
  build) build ;;
  publish) publish "${2:-}" ;;
  *) fail "usage: scripts/release.sh build | publish <version>" ;;
esac
