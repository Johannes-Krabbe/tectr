#!/usr/bin/env bash
# Archives a Release build of the iOS app and uploads it to App Store Connect, where it shows up
# in TestFlight once Apple has processed it (10 to 30 minutes). It runs on this Mac, so uploads
# cost no CI minutes. Signing is automatic, with the Apple account signed in to Xcode.
#
# It ships the working tree as it is, uncommitted changes included, and rebuilds the engine
# first, so the upload never carries a stale one.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

# Use Xcode even when xcode-select still points at the Command Line Tools.
if ! xcodebuild -version >/dev/null 2>&1 && [ -d /Applications/Xcode.app ]; then
  export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
fi
if ! xcodebuild -version >/dev/null 2>&1; then
  echo "ios-testflight: Xcode not found. Install it from the App Store, then run this again." >&2
  exit 1
fi

# The app's version is tectr's, so a TestFlight build says which release it matches.
version=$(sed -n -E 's/^version = "([^"]+)"$/\1/p' Cargo.toml | head -n 1)
# Build numbers only have to grow, so a UTC timestamp works without keeping count anywhere.
build_number=$(date -u +%Y%m%d.%H%M)

scripts/ios-core.sh
cd apps/ios
xcodegen generate --quiet

out=.build/testflight
rm -rf "$out"

# Unsigned: the export signs it for distribution, so the archive doesn't need a development
# certificate. The app has no entitlements, so nothing is lost by archiving it unsigned.
xcodebuild \
  -project Tectr.xcodeproj \
  -scheme Tectr \
  -configuration Release \
  -destination 'generic/platform=iOS' \
  -derivedDataPath .build/DerivedData \
  -archivePath "$out/Tectr.xcarchive" \
  -quiet \
  archive \
  MARKETING_VERSION="$version" \
  CURRENT_PROJECT_VERSION="$build_number" \
  CODE_SIGNING_ALLOWED=NO

xcodebuild \
  -exportArchive \
  -archivePath "$out/Tectr.xcarchive" \
  -exportPath "$out" \
  -exportOptionsPlist ExportOptions.plist \
  -allowProvisioningUpdates \
  -quiet

echo "Uploaded $version ($build_number). It shows up in TestFlight once processed."
