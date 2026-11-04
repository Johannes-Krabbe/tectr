# tectr

A fast desktop and mobile app for running, watching, and steering many coding agents in parallel. See [idea.md](idea.md) for the product and [context/architecture.md](context/architecture.md) for the design.

Today this is the first milestone, a daemon and a command-line client, plus two empty apps that aren't connected to anything yet.

## Requirements

- Rust (the version is pinned in `rust-toolchain.toml`)
- Git
- Node 22 or newer and npm, to run Claude Code sessions
- A working Claude Code login on the machine running the daemon

## Try it

Start the daemon. It listens on loopback only:

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
```

To use several daemons, repeat `--daemon` or set `TECTR_DAEMONS` to a comma-separated list of URLs.

Permission modes are `manual`, `auto` (the default), and `yolo`. `yolo` never asks before running anything.

## Develop

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

The tests need no Claude login. They use `fake-agent`, a scripted ACP agent. `mise.toml` has the same commands as tasks. These commands skip the desktop app. Avoid `--workspace`, which includes it and needs its frontend built first.

## Empty apps

These only show a placeholder so far.

Desktop (Tauri), which needs pnpm:

```sh
pnpm install
cd apps/dsk && pnpm tauri dev
```

iOS (SwiftUI), which needs Xcode and [XcodeGen](https://github.com/yonaskolb/XcodeGen). The Xcode project is generated and isn't in git:

```sh
cd apps/ios && xcodegen generate
open Tectr.xcodeproj
```
