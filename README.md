<h1>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="design/logo/lockup-on-dark.svg">
    <img alt="tectr" src="design/logo/lockup-on-light.svg" height="40">
  </picture>
</h1>

> [!CAUTION]
> **I don't recommend using tectr yet.** It is in early development and essential features are still missing. Expect breaking changes, rough edges and lost data.

A fast desktop and mobile app for running, watching, and steering many coding agents in parallel. See [idea.md](idea.md) for the product and [context/architecture.md](context/architecture.md) for the design.

Today this is a daemon, a command-line client, a first version of the desktop app, and an empty iOS app.

## Install

On an Apple Silicon Mac:

```sh
brew trust johannes-krabbe/tap    # Homebrew only loads casks from taps you trust
brew install --cask johannes-krabbe/tap/tectr
```

This installs the app, the `tectr` CLI and `tectr-daemon`, and runs the daemon as a LaunchAgent, so it starts at login and keeps agents running when the app is closed. While an agent works it also keeps the Mac from idle sleeping, so locking the screen doesn't pause it (closing the lid still does). Turn that off in the device's settings or with `tectr keep-awake off`. `brew upgrade --cask tectr` restarts it on the new version. Its log is `~/Library/Logs/tectr/daemon.log`. It reads your login shell's PATH, so agents installed through nvm or into `~/.local/bin` are found. Claude Code sessions need Node 22 or newer.

## Requirements

- Rust (the version is pinned in `rust-toolchain.toml`)
- Git
- Node 22 or newer and npm, to run Claude Code sessions
- A working Claude Code login on the machine running the daemon

## Try it

Anything built from source is a dev build: its daemon uses port 7769, `~/Library/Application Support/tectr-dev` and `~/.config/tectr-dev`, so it runs next to the installed app without sharing anything. Only `mise run release` builds use the installed app's port 7767 and `tectr` folders.

Start the daemon. Its websocket listens on loopback only, and other machines reach it over iroh once paired (see below):

```sh
cargo run -p daemon
```

In another terminal, add a repo and start a session. Commands that return an ID accept any unique prefix of it:

```sh
alias tectr="cargo run -q -p cli --"

tectr add-workspace /path/to/repo      # prints the workspace ID
tectr new-topic <workspace> "Fix login" # creates a branch and worktree
tectr new-session <topic> --mode manual --model haiku
tectr chat <session>
```

In `chat`, type a prompt to send it, or an option's number when the agent asks for permission.

Other commands:

```sh
tectr status                           # devices, workspaces, topics, sessions, approvals
tectr prompt <session> "..."           # send a prompt and return
tectr watch <session> [--until-done]   # print the history and follow it
tectr answer <approval> <option>       # answer from anywhere, by number or ID
tectr cancel <session>                 # interrupt the current turn
tectr rename-topic <topic> "..."       # choose a topic's name
tectr move-topic <topic> new|root|<worktree> [--force]  # move its tabs to another folder
tectr archive-topic <topic> [--force]  # stop its agents, remove its worktrees, keep its tabs
tectr archived                         # list archived topics
tectr restore-topic <topic>            # bring one back with its folder
tectr rename-device "..." [--on <device>]  # name a device, "" goes back to its hostname
tectr mcp ls                           # MCP servers, and which ones each topic gets
tectr mcp add github --env 'TOKEN=${TOKEN}' -- npx -y some-server  # a stdio server
tectr mcp add linear --url https://mcp.linear.app/mcp [--topic <topic>]  # an HTTP server
tectr mcp off github --topic <topic>   # also: on, default, rm, login, logout, import
```

Topics can be named by their exact name or an ID prefix. Single-quote `${VAR}` values so the daemon, not your shell, fills them in.

To reach a daemon on another machine, pair with it. No VPN or port forwarding is needed: connections go over [iroh](https://iroh.computer), directly when the network allows and otherwise through n0's public relays, end-to-end encrypted either way.

```sh
tectr pair                             # on the machine to reach: prints a code that works for 5 minutes,
                                       # then asks for the digits the other machine shows
tectr add-device <code>                # on the machine that wants in: shows the digits to type there
tectr unpair <client> [--on <device>]  # take a machine off a device's allowed list
tectr remove-device <device>           # stop reaching a paired device
```

In the desktop app, the same is "Pair another machine" and "Add a device" in the palette, in the menu next to Settings and in Settings under Devices, and a device's Pairing settings.

Without `--daemon`, the CLI and the app use this machine's daemon and every device paired from this machine. To choose daemons yourself, repeat `--daemon` or set `TECTR_DAEMONS` to a comma-separated list of URLs.

Permission modes are `manual`, `auto` (the default), and `yolo`. `yolo` never asks before running anything.

## Develop

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

The tests need no Claude login. They use `fake-agent`, a scripted ACP agent. `mise.toml` has the same commands as tasks. These commands skip the desktop app. Avoid `--workspace`, which includes it and needs its frontend built first.

## Desktop app

The desktop app (Tauri, needs pnpm) does everything the CLI does. Start a daemon as above, then:

```sh
pnpm install
cd apps/dsk && pnpm tauri dev
```

It connects to the daemon on this machine, or to the ones in `TECTR_DAEMONS`. ⌘K, or the row at the top of the sidebar, lists every action with its shortcut.

The app's own settings live in `~/.config/tectr/dsk.toml` (`~/.config/tectr-dev/dsk.toml` when built from source), which holds only what differs from the defaults and applies as soon as it's saved. `dsk.defaults.toml` next to it lists every setting. The file starts with `version`, which says which layout it's in: the app writes it, and brings a file from an older tectr up to date when it reads it, keeping what it was as `dsk.v1.toml`. Settings (⌘,) edits the same file, so people and agents can both change it. Shortcuts are the first section:

```toml
[keybinds.nav]                # the shared default for moving around
down = ["n", "down"]          # everything that follows nav.down moves with it
input_down = ["ctrl+j", "down"]   # the same, for lists you search by typing

[keybinds.sidebar]
down = "s"                    # only the sidebar

[keybinds.global]
new_topic = "mod+g"           # mod is cmd on macOS, ctrl elsewhere
interrupt = []                # no key

[keybinds.permission]
allow_once = { key = "y", hold_ms = 500 }   # has to be held for half a second
reject = { key = "n", repeat = true }       # keeps firing while held

[keybinds.pane]
first = "g g"                 # a sequence, which works the way it does in Vim
```

Appearance is in the same file, and Settings shows each change as it happens. Moving through a list of themes or fonts tries each one out before you choose:

```toml
[appearance]
mode = "system"               # or light, or dark
dark_theme = "tokyo-night"    # one theme for each mode
light_theme = "github-light"

[appearance.text]
family = "IBM Plex Sans"      # any installed font
size = 14

[appearance.code]
family = "JetBrains Mono"
size = 13
ligatures = false

[appearance.layout]
density = 90                  # also corners, sidebar_width and content_width

[appearance.themes.midnight]  # your own theme, which then shows up in the lists
extends = "tokyo-night"
"surface.base" = "#000000"
```

Every other page of Settings has a section of its own:

```toml
[general]
show_shortcuts = true         # a line along the bottom with the keys that work right now

[conversation]
steps = "shown"               # tool calls and thinking, instead of folded into one line

[terminal]
size = 12
cursor = "bar"

[vim]
enabled = true                # vim keys in the message box and the new topic box
escape = "jk"                 # leave insert mode with jk as well as Esc
system_clipboard = false      # keep yanks out of the system clipboard
```

The app comes with its own two themes plus Catppuccin, Tokyo Night, Dracula, One, GitHub, Nord, Gruvbox, Solarized, Rosé Pine, Monokai and Vague, in dark and light where they have both.

To start topics from the app, add a provider first: hover a device in the sidebar and click the settings icon, or press Enter on it. Adding one starts that agent once on that device to ask which models and modes it offers.

## Release

Releases are built and published from this Mac, not CI. `mise run release:build` builds and packages into `target/release-dist` without publishing. `mise run release 0.2.0` bumps the version, builds, tags, pushes, creates the GitHub release and updates the cask in `johannes-krabbe/homebrew-tap`. Edit the cask in `packaging/homebrew/tectr.rb`, not in the tap.

## iOS app

This only shows a placeholder so far. It's SwiftUI, and needs Xcode and [XcodeGen](https://github.com/yonaskolb/XcodeGen). The Xcode project is generated and isn't in git:

```sh
cd apps/ios && xcodegen generate
open Tectr.xcodeproj
```
