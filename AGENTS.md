We don't care about backwards compatibility, this app is only used by me and i am fine with always updating to the latest version. So feel free to remove any unused code even if it breaks older versions.

**Hard rules:**

- **Writing:** no em dashes, ellipsis characters, middots, arrows or decorative unicode anywhere (chat, code, docs, commits, UI copy). Use plain ASCII, `...` and `->`. Exception: platform key glyphs like the Cmd symbol in the UI.
- **Commits:** only commit when I specifically ask. Then commit to the current branch, usually main, and stage only the requested change. Never open a PR unless asked.
- **No services unless asked:** don't start the daemon, the app or simulators. Stop anything you started.
- **Never validate visually on your own.** Only do it if I ask, or ask me to do it.

# tectr

A fast desktop and mobile app for running, watching and steering many coding agents in parallel, across your own machines, with no central server. It is a **keyboard-first, highly customizable power user tool**.

Today: daemon, CLI and a first desktop app, with pairing and remote connections over iroh. iOS is a placeholder. The push relay and diffing aren't built.

## Power user rules

- Every action has a rebindable key and a palette entry. Nothing is mouse-only.
- New behavior and appearance choices are settings: `~/.config/tectr/dsk.toml`, with defaults in `dsk.defaults.toml`. The Settings UI and the file stay equal, and changes apply on save.
- Favor speed and density over hand-holding: no onboarding or explanatory filler.

## Product model

Device -> workspace -> topic -> tabs. A workspace lives on one device. The daemon owns all state, each client keeps its own view, and closing a tab kills its agent.

## Structure

| Path | What |
| --- | --- |
| `crates/protocol` | Types shared by the daemon and `engine` |
| `crates/daemon` | Runs on each machine, owns its state, spawns agents over ACP |
| `crates/engine` | Client logic shared by the apps |
| `crates/cli` | Command-line client |
| `crates/settings` | Desktop settings |
| `crates/highlight` | Syntax highlighting |
| `crates/fake-agent` | Scripted ACP agent for tests |
| `apps/dsk` | Tauri + React desktop app |
| `apps/ios` | SwiftUI app |
| `design/` | Tokens, icons, component specs |

## Invariants

- **Thin clients:** apps render state and send actions. They never touch a socket or talk to an agent. Shared logic goes in `engine`.
- **Loopback or a paired key:** the daemon's websocket stays on a hardcoded loopback address, and no setting may change it. Remote clients connect only over iroh, and only with a key on the daemon's allowed list. Anything else would effectively be remote code execution.
- **Speed is a feature:** instant startup, batched per-frame updates, deltas rather than whole transcripts, virtualized transcripts, heavy work off the UI thread.
- **Tokens:** app code uses tokens, with no raw colors. Edit `design/tokens/*.tokens.json`, never `generated/`, then run `mise run tokens`.
- **`unsafe`:** forbidden except `color_title_bar`. Ask before adding another.
- **Device settings** live in the daemon. Adapters start only after the user adds them. Never hardcode model lists.

## Design

Calm and Linear-adjacent. Real macOS title bar tinted to the background, with no HTML or overlay bars. Don't assume a palette: users choose their own color theme.

## Commands

```sh
mise run test         # cargo test
mise run lint         # fmt check + clippy
mise run dsk:test     # desktop app tests
mise run tokens:test  # tokens, contrast, no raw colors
mise run tokens       # regenerate tokens and icons
mise run logo         # regenerate logo files and app icons
mise run release:build  # build the release app, CLI and daemon locally, publishing nothing
```

Builds from source are the dev channel (port 7769, `tectr-dev` folders) and never touch the installed release. Run `mise run release <version>` only when asked: it publishes.

Avoid `cargo --workspace`: it includes the desktop app, which needs its frontend built first.

Verify changes with the relevant suites and lint. New behavior gets tests, using `fake-agent` for anything agent-related (no Claude login needed).

## Docs

Read these when a task is in their area:

- `idea.md`: product
- `context/architecture.md`: how it's built and why
- `context/design-system.md`, `design/README.md`: design system

If a change alters a documented decision, rewrite that text in the same change and keep its status label (Settled, Proposed, Built).

## Taste

- Work out the smallest change that works and where it might fall short. If it differs from the maintainable long-term change, ask me.
- Comments explain why, not what.
- TypeScript: no `as` casts, no `any`.
