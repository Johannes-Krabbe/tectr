---
name: ios-install
description: Build the tectr iOS app from the current checkout and install it on Johannes's iPhone over USB or Wi-Fi. Use when the user asks to install, put or update the app on their phone.
disable-model-invocation: true
---

# Install the iOS app on the phone

Builds a signed Release build of `apps/ios` and installs it with `devicectl`. Install only:
never launch the app (AGENTS.md: no services unless asked).

## Known values

| What | Value |
| --- | --- |
| Phone | iPhone 17 Pro Max, named `iPhone` |
| Phone UDID | `00008150-001623480C07801C` |
| Signing identity | `Apple Development: Johannes Krabbe (X63L5QZTJG)` |
| Team ID | `V23RY29ZXJ` (the certificate's OU, not the id in parentheses) |
| Bundle ID | `dev.tectr.ios` |
| Build output | `target/ios-device/Build/Products/Release-iphoneos/Tectr.app` |

## Gotcha: DEVELOPER_DIR

`xcode-select` points at the Command Line Tools, which have no `devicectl` and no iOS SDK.
Prefix every `xcrun` and `xcodebuild` call with
`DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer`. `scripts/ios-core.sh` already
does this for itself.

## Steps

Run from the repo root.

1. Check the phone is reachable. It should show `available (paired)`:

   ```sh
   DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer xcrun devicectl list devices
   ```

   If the UDID differs or the phone is missing, use the one listed and tell the user. If it
   is not paired or unavailable, ask the user to plug it in and unlock it.

2. Build the engine. Needed after any Rust change, and cheap when nothing changed:

   ```sh
   mise run ios:core
   ```

3. Generate the Xcode project (it is gitignored and has no team set):

   ```sh
   (cd apps/ios && xcodegen generate)
   ```

4. Build, passing the team on the command line because `project.yml` has none:

   ```sh
   cd apps/ios && DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer xcodebuild \
     -project Tectr.xcodeproj -scheme Tectr -configuration Release \
     -destination 'id=00008150-001623480C07801C' \
     -derivedDataPath ../../target/ios-device \
     DEVELOPMENT_TEAM=V23RY29ZXJ CODE_SIGN_STYLE=Automatic -allowProvisioningUpdates \
     build 2>&1 | grep -E 'error|BUILD' | tail -20
   ```

   Expect `** BUILD SUCCEEDED **`. On failure, rerun without the grep to see the full error.

5. Install:

   ```sh
   DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer xcrun devicectl device install app \
     --device 00008150-001623480C07801C \
     target/ios-device/Build/Products/Release-iphoneos/Tectr.app
   ```

   Expect `App installed:` with `bundleID: dev.tectr.ios`.

## Troubleshooting

- **App won't open on the phone:** Settings > General > VPN & Device Management, trust the
  developer certificate.
- **App stopped opening after about a week:** a free personal team's provisioning profile
  lasts 7 days. Reinstall with these steps.
- **Signing errors about no account or profile:** Xcode needs the Apple ID signed in under
  Xcode > Settings > Accounts. Ask the user to check; don't try to fix it yourself.
- **Install fails with the device locked or busy:** ask the user to unlock the phone and retry.

Nothing here changes tracked files: the Xcode project, the xcframework and `target/` are all
gitignored. Don't commit anything unless asked.
