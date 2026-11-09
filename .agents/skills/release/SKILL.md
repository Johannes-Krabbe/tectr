---
name: release
description: Build tectr locally and publish a new version to GitHub Releases and the Homebrew tap (johannes-krabbe/tap). Only run when the user asks for a release.
argument-hint: "[version, for example 0.2.0]"
disable-model-invocation: true
---

# Release tectr

This publishes a new version of tectr. Everything runs on this Mac: the build, the
GitHub release upload, and the cask update. No CI is involved.

Requested version: `$ARGUMENTS` (may be empty).

Follow the steps in order. Do not skip steps, and do not improvise. If a step says
stop, stop and tell the user what happened, with the exact error output.

## What the release does

`scripts/release.sh publish <version>` does all of this in one run:

1. Checks that you're on `main`, the tree is clean, `origin` exists, the tag is new,
   `gh` is logged in, and the tap `johannes-krabbe/tap` is tapped and clean. It then
   pulls the tap.
2. Writes the version into `Cargo.toml`, `apps/dsk/package.json` and
   `apps/dsk/src-tauri/tauri.conf.json`, and refreshes `Cargo.lock`.
3. Builds with `TECTR_CHANNEL=release`: the desktop app (`pnpm --dir apps/dsk tauri build`),
   then `tectr` and `tectr-daemon` (`cargo build --release`). It refuses to continue if the
   daemon isn't a release build (default port 7767).
4. Packages `target/release-dist/tectr_<version>_darwin_arm64.tar.gz` with `tectr.app`,
   `tectr` and `tectr-daemon`.
5. Commits `Release v<version>` (only if the version changed), creates the annotated tag
   `v<version>`, and pushes `main` and the tag to `origin`.
6. Creates the GitHub release `v<version>` on `Johannes-Krabbe/tectr` with the tarball and
   generated notes.
7. Renders `packaging/homebrew/tectr.rb` with the version and sha256 into
   `$(brew --repo johannes-krabbe/tap)/Casks/tectr.rb`, then commits and pushes the tap.

## Rules

- Never install, upgrade or start tectr, its daemon or the app. Installing the cask starts a
  LaunchAgent. The user does that themselves.
- Never edit `Casks/tectr.rb` in the tap by hand except in the recovery steps below. The
  template is `packaging/homebrew/tectr.rb`.
- Never force-push, delete tags on `origin`, or delete GitHub releases without asking first.
- Writing rule for anything you write (commit messages, notes): plain ASCII only. No em
  dashes, no ellipsis character, no arrows. Use `...` and `->`.
- `mise` may not be on PATH in your shell. Call the scripts and tools directly, as shown.
- Run every command from the repo root: `/Users/johanneskrabbe/Developer/tectr`.

## Step 1: Choose the version

1. Find the last release: `git describe --tags --abbrev=0` (for example `v0.1.0`).
2. If `$ARGUMENTS` is a version like `0.2.0` (three numbers, no `v`), use it. It must be
   higher than the last release. If it isn't, stop and ask.
3. If `$ARGUMENTS` is empty, list what changed with
   `git log <last-tag>..HEAD --oneline` and propose a version:
   - While the version is below 1.0.0: new features or breaking changes -> bump the middle
     number (0.1.3 -> 0.2.0). Only fixes and small changes -> bump the last number
     (0.1.3 -> 0.1.4).
   - Ask the user to confirm the version before continuing. Do not continue without an answer.
4. If `git log <last-tag>..HEAD --oneline` prints nothing, there's nothing to release. Stop
   and tell the user.

## Step 2: Check the preconditions

Run these and check each result:

```sh
git branch --show-current          # must print: main
git status --porcelain             # must print nothing
git fetch origin && git status -sb | head -1   # must not say "behind"
gh auth status                     # must say Logged in
git -C "$(brew --repo johannes-krabbe/tap)" status --porcelain   # must print nothing
uname -sm                          # must print: Darwin arm64
```

- Not on `main`: stop and ask.
- Uncommitted changes: stop. Show `git status --short` to the user and ask whether to commit
  them first. Do not commit or stash them on your own.
- Behind `origin/main`: stop and ask. Do not pull or rebase on your own.
- `gh` not logged in: stop and tell the user to run `! gh auth login`.
- Tap has changes: stop and show `git -C "$(brew --repo johannes-krabbe/tap)" status --short`.

## Step 3: Run the tests and lint

Do not release a version that fails these. Run each one and wait for it to finish:

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo clippy -p dsk --all-targets -- -D warnings
cargo test
node --experimental-strip-types --test apps/dsk/src/*.test.ts
node --test design/tokens/tokens.test.mjs
```

If any of them fails, stop and show the failing output. Do not try to fix the code as part of
the release.

## Step 4: Publish

Run this, with a timeout of 600000 ms, because the build takes several minutes:

```sh
scripts/release.sh publish <version>
```

Success ends with these two lines:

```
https://github.com/Johannes-Krabbe/tectr/releases/tag/v<version>
Released v<version>. Install or upgrade with: ...
```

If it fails, find the step where it failed in the output and go to "Recovery" below.

## Step 5: Verify

Run all of these. Each must succeed.

```sh
git status -sb | head -1        # must show "## main...origin/main" with no ahead or behind
git tag --points-at HEAD        # must include v<version>
git ls-remote --tags origin v<version>          # must print one line
gh release view v<version> --repo Johannes-Krabbe/tectr --json tagName,assets -q '.tagName, .assets[].name'
#   must print v<version> and tectr_<version>_darwin_arm64.tar.gz
TAP="$(brew --repo johannes-krabbe/tap)"
git -C "$TAP" status -sb | head -1              # must show no ahead or behind
grep -E '^  (version|sha256) ' "$TAP/Casks/tectr.rb"
```

Then check that the uploaded file matches the cask. Download it into the session's scratchpad
directory (not the repo):

```sh
curl -sL -o <scratchpad>/tectr.tgz \
  https://github.com/Johannes-Krabbe/tectr/releases/download/v<version>/tectr_<version>_darwin_arm64.tar.gz
shasum -a 256 <scratchpad>/tectr.tgz
```

The hash must equal the `sha256` in `Casks/tectr.rb`. If it doesn't, stop and tell the user.

## Step 6: Report

Tell the user, briefly:

- The version released and the release URL.
- The commits it contains (the `git log <last-tag>..v<version> --oneline` list).
- That verification passed.
- The command to upgrade, which they run themselves:

```sh
brew update && brew upgrade --cask tectr
```

- That upgrading restarts the daemon, so running agents stop and resume on their next prompt.

## Recovery

Find the last thing the script printed, then use the matching case. After a fix, run Step 5.

**Failed before or during the build** (`release: ...` check messages, pnpm, tauri or cargo
errors, or "the daemon isn't a release build"). Nothing was committed, tagged or published.
Undo the version bump and stop:

```sh
git checkout -- Cargo.toml Cargo.lock apps/dsk/package.json apps/dsk/src-tauri/tauri.conf.json
git status --porcelain   # must print nothing
```

Show the user the error. Do not retry with changes to the code.

**`git push` failed** (the commit and tag exist locally, nothing on GitHub). Show the error. If
it's a network or auth problem the user has fixed, push and continue by hand:

```sh
git push origin main v<version>
```

Then do the "GitHub release failed" steps below.

**GitHub release failed** (`gh release create` error, but the tag is on `origin`). The tarball
is still in `target/release-dist/`. Create the release, then do the "Tap update failed" steps:

```sh
gh release create v<version> target/release-dist/tectr_<version>_darwin_arm64.tar.gz \
  --repo Johannes-Krabbe/tectr --title v<version> --generate-notes
```

If it says the release already exists, check it with `gh release view v<version> --repo
Johannes-Krabbe/tectr`. If the tarball is missing from it, upload it with
`gh release upload v<version> target/release-dist/tectr_<version>_darwin_arm64.tar.gz --repo Johannes-Krabbe/tectr`.

**Tap update failed** (the release exists, the cask wasn't committed or pushed):

```sh
TAP="$(brew --repo johannes-krabbe/tap)"
SHA="$(shasum -a 256 target/release-dist/tectr_<version>_darwin_arm64.tar.gz | cut -d ' ' -f 1)"
sed -e "s/@VERSION@/<version>/" -e "s/@SHA256@/$SHA/" packaging/homebrew/tectr.rb > "$TAP/Casks/tectr.rb"
git -C "$TAP" add Casks/tectr.rb
git -C "$TAP" commit -m "tectr <version>"
git -C "$TAP" push
```

If the tap's commit exists but the push failed, only run `git -C "$TAP" push`.

**Anything else, or a case you're not sure about:** stop and tell the user exactly what was
printed and what state things are in (local commit and tag, `origin`, GitHub release, tap).
Do not delete tags or releases to start over unless the user says so.
