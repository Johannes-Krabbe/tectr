# tectr: architecture and design decisions

Last updated: 2026-11-04. The first milestone is built: `protocol`, `daemon`, `engine`, `cli`, and `fake-agent`. `apps/dsk` is connected to `engine` and matches the CLI's features. Everything else here is still a design, not a description of a working system.

`idea.md` describes the product. This file records how it's built and why.

Each decision has a status:

- **Settled**: stated in `idea.md` or explicitly agreed.
- **Proposed**: recommended and not yet explicitly confirmed. Change it freely.
- **Built**: decided while building the first milestone, and covered by tests. Not separately confirmed, so change it if you disagree.

## Overview

tectr runs many coding agents across several machines, without a central server in the data path. Each machine runs a daemon that owns its work. Each client connects directly to every daemon and merges the results into one view.

```
 ┌──────────────┐             ┌──────────────┐
 │ Desktop app  │             │ iOS app      │
 │ (Tauri)      │             │ (SwiftUI)    │
 ├──────────────┤             ├──────────────┤
 │ engine       │             │ engine       │
 └──┬───────┬───┘             └──┬─────┬─────┘
    │       └───────────┐ ┌──────┘     │
    │ loopback          │ │ Tailscale  │    websocket (tectr event schema)
 ┌──┴─────────┐      ┌──┴─┴───────┐  ┌─┴──────────┐
 │ daemon     │      │ daemon     │  │ daemon     │
 │ (laptop)   │      │ (desktop)  │  │ (VPS)      │
 └──┬─────────┘      └──┬─────────┘  └─┬──────────┘
    │ ACP (JSON-RPC over stdio)
    │
 claude-agent-acp, codex-acp, other ACP agents
```

Push notifications are the one exception to the direct connections, and they carry only encrypted payloads:

```
daemon ──encrypted payload──► push relay ──► APNs ──► iOS notification extension (decrypts)
```

## Domain model

```
Device (machine_id)
└── Workspace        a repo on one device
    └── Topic        a feature or task
        ├── Worktree (many)
        ├── Session (many)    an agent run; points at one worktree
        └── Terminal (many)
```

| Decision | Status | Why |
|---|---|---|
| A workspace belongs to one device, so the same repo on two devices is two workspaces | Settled | Each daemon stays the single source of truth for its own work |
| A topic can span many branches and worktrees | Settled | Parallel work within a task stays isolated |
| The daemon owns all state, including tabs, and each client has its own view of it | Settled | Every client sees the same topics, tabs, agents and drafts, and moving around on one doesn't move the others. See Shared state and separate views |
| A session points at a worktree, with no 1:1 constraint in the schema | Proposed | Either answer to "can agents share a worktree?" remains possible. One-at-a-time can be a rule added later |
| Agent and permission mode are stored per session | Proposed | The agent is chosen per tab, and sessions can run in different modes |
| IDs are scoped by machine, such as `(machine_id, session_id)` | Settled | Clients merge many daemons without collisions |

## Components

### Daemon (`daemon`)

Runs on each machine. It owns workspaces, topics, sessions, and terminals, spawns agents in their worktrees, and exposes one stable event schema over a websocket.

- **Source of truth (Settled):** clients can come and go without losing anything.
- **Persistence in SQLite from day one (Proposed):** workspaces, topics, and session events must survive a daemon restart. Resume also depends on stored events.
- **Loopback only until pairing exists (Proposed):** the daemon can drive agents and will later expose terminals, so an unauthenticated network listener is effectively remote code execution. The address is hardcoded, so no setting can change it.
- **Browsers are rejected (Built):** any web page can open a websocket to loopback, so the daemon refuses any connection with an `Origin` header. Native clients don't send one.
- **Other users on the same machine aren't blocked yet (Known gap):** loopback has no authentication, so on a shared machine another local user could connect and run agents as you. Don't run the daemon on a shared machine until pairing exists.
- **Crash recovery (Built):** agent processes don't survive the daemon. On startup, sessions that were starting, working, or waiting become `stopped`, and open approvals are resolved as cancelled.
- **Worktrees live in the data directory (Proposed):** `<data dir>/worktrees/<workspace>/<slug>-<id>`, on a new branch `tectr/<slug>-<id>` from the repo's `HEAD`. This keeps the user's repo folder clean.
- **Topics at the repo root (Proposed):** a topic can work in the repo's own folder instead, and the desktop new topic form starts on this choice. It's stored as a worktree record pointing at the workspace path, so sessions need no special case. Its branch is recorded at creation and can go stale if the repo switches branches. Deleting a topic never removes one of these.
- **Device settings (Settled):** settings specific to a device live in its daemon, in SQLite, so every client sees the same ones. Client settings stay on the client (see Desktop settings). Providers are the first device setting.
- **Providers (Proposed):** the daemon knows how to run Claude Code, Codex, and OpenCode, plus anything passed with `--agent`. Checking which are available only looks on `PATH` and starts nothing. An agent starts only once it's added to that device. It starts in an empty folder with no prompt, and the daemon stores the modes, models, and per-model effort levels it reports. Nothing is hardcoded, so new models need no app update. Added providers are asked again each time the daemon starts, and one that fails keeps what it last reported. OpenCode is the exception and only updates on Add or Refresh, because each time it's asked it adds an empty "New session" to the person's OpenCode history. Each provider has a default model and effort, chosen in the device's settings and kept when it's asked again. "Default" entries from agents are never offered, so what's shown is what runs. Only a model the agent itself names as recommended is marked, as Claude does. No agent says which effort it recommends, so none is marked. A provider is only a list for pickers: `start_session` doesn't require one, so the CLI and `--agent` setups keep working.
- **Topic names (Proposed):** a topic created without a name is numbered within its workspace, like "Topic #3", and goes by its first open tab's title once that has one, following later changes to it. The daemon does this, so every client sees the same name. Choosing a name, on creation or with `rename_topic`, stops it for good, and the branch and worktree keep the name they were made with.
- **Deleting a topic (Proposed):** `delete_topic` stops the topic's agents and removes it from the app. Like a closed session, it stays in the database with its events. Its worktree is removed afterwards, once the agents have exited, so the command returns straight away. Git decides what's safe: `git worktree remove` and `git branch -d` are never forced, so a worktree with uncommitted changes stays on disk with its branch, and a branch with unmerged commits stays in the repo. Nothing tells the person when that happens beyond what the confirmation says beforehand and a line in the daemon's log, and nothing cleans those up later. The `force` flag only confirms stopping running agents.

### Desktop settings (`settings`)

- **One file (Settled):** `~/.config/tectr/dsk.toml` holds every setting of the desktop app, one section per area, currently `[keybinds]` and `[appearance]`. It's TOML because people and agents both edit it, so it needs comments, and `toml_edit` can change one value while keeping the rest as written.
- **Only what differs (Settled):** defaults live in the `settings` crate, so the file stays short enough for an agent to edit by line. `dsk.defaults.toml` next to it is rewritten on launch and lists every setting and its default. It's the reference, and is never read back.
- **Everything is configurable (Settled):** every key in the app is an action in `crates/settings/src/keybinds.rs`, including permission answers, arrows, and whether a key has to be held (`hold_ms`) or repeats (`repeat`). `apps/dsk/src/keybinds.ts` lists the same IDs, which a test checks. What's left to the system: editing keys inside text fields, Enter and Space on a focused button, and what the macOS menu takes first, which the file warns about.
- **Shared navigation with overrides (Settled):** `[keybinds.nav]` is the default for moving around. Actions follow it until they're set themselves, so changing `nav.down` moves everything except what was set on its own. Lists you search by typing, which today means the palette and pickers, follow `nav.input_up` and `nav.input_down` instead, because letters there have to reach the box. Those default to Ctrl+K and Ctrl+J on macOS. Elsewhere `mod` is Ctrl, and Ctrl+K toggles the palette, so they default to only the arrows.
- **Vim-inspired keys (Settled):** Vim's keys are the default wherever an action means the same thing there, such as `gg`, `G`, `zz`, `gt`, `dd`, `Ctrl+D`. What Vim has no word for keeps its ⌘ key, and the ⌘ and arrow keys stay alongside the Vim ones.
- **Sequences work like Vim's (Built):** a key that starts a sequence waits for the next until `keybinds.sequence_timeout_ms`, and a key that doesn't continue it drops what came before and acts on its own. Two differences: a key that also starts a longer sequence fires straight away rather than waiting, which the settings screen warns about, and keys that type can't be part of a sequence in a text box, so `jk` to leave the message box doesn't work.
- **A bad file never costs the keyboard (Built):** a syntax error keeps the last good settings. A bad entry falls back to its default. Both are shown on the settings screen, which refuses to save over a file that doesn't parse.
- **The webview can't touch the file (Built):** Rust owns it and exposes only `settings`, `set_keybind`, `reset_keybind`, `set_appearance` and `reset_appearance`, each of which checks what it's given, because the webview renders agent output. `try_window_theme` is the one other thing it can do to the app's look, and it only names a theme that exists and saves nothing.
- **Appearance is rows from a table (Built):** `FIELDS` in `crates/settings/src/appearance.rs` holds each setting's default and what it can be. The screen draws its rows from that, so a new setting there needs only a line in `cssVariables` in `apps/dsk/src/appearance.ts`. Rust works out every theme's colors, and the webview only turns values into CSS variables on `<html>`. The native title bar is the one thing it can't draw, so Rust also sets the window's color and whether macOS draws for dark or light, including for a theme being tried out in a list.
- **Themes are data (Built):** themes that come with the app are in `crates/settings/src/themes.toml`, in the format of a theme in the settings file. A theme sets what it wants and the rest follows from rules, so changing an accent changes the tints made from it. Tectr's own two have to match `design/tokens`, which a test checks against `design/tokens/generated/desktop-defaults.json`.
- **A theme belongs to one mode (Settled):** there's one theme chosen for dark and one for light, and a list only offers themes of its mode. A light theme in `dark_theme` would leave the title bar's text unreadable, so the file falls back to the default and says so.
- **The shortcut line (Built):** `appearance.layout.show_shortcuts`, off by default, draws a line along the bottom of the window with the keys that work right now. It's a client setting because it's about this keyboard, and it has to work with no daemon reachable. Each screen declares its hints with `useHints` next to the handlers that run those keys, leaving out what would do nothing, and `chooseHints` in `apps/dsk/src/shortcuts.ts` picks by what has the keyboard: an overlay hides everything behind it, and otherwise it's the sidebar row, the pane or the text field, followed by the global keys. A hint names actions rather than keys, so it follows rebinding, and `formatKeys` shows actions that belong together as one label like `K/J` or a range like `Cmd 1-9`. What doesn't fit is dropped as whole hints from the end, except the pinned palette and next request keys.
- **A launch starts from what the last one showed (Built):** the webview keeps the last appearance in local storage and paints it before anything else, so a theme doesn't flash the default on the way in. It's a copy, so losing it costs one flash.

### Shared state and separate views (Settled)

The daemon owns what exists, and each client chooses what to look at. Every client sees the same topics, tabs, agents, permission requests and unsent drafts, and opening a topic on one doesn't move the others.

- **Which tabs exist is shared, and which one is showing isn't.** A new tab appears everywhere, and only the client that started it switches to it. If another client closes the tab you're on, you move to the next one, or else the previous one.
- **Drafts are shared.** `DaemonState.drafts` is stored in SQLite and sent to every client, so a message started on one can be finished on another. The last write wins.
- **Closing a tab stops its agent.** `close_session` kills the process, removes the session from state and drops its draft, keeping its events in the database. It refuses a running agent without `force`, so the second confirmation holds for every client, not only the ones that ask.
- **Navigation stays on the client.** `apps/dsk/src/navigation.ts` holds the current screen, the selected tab in each topic, where going back leads, the last topic per workspace and the settings section. It's kept in the webview's local storage, so a restarted client opens where it was and a new one opens on the empty state. Losing it costs nothing but the place you were. It needs no daemon, so a client with none reachable can still open its own settings, and a view can name an offline device.
- **The new topic form stays on the client and isn't saved.** It lives in memory until the topic starts or the form is cancelled, and restarting clears it.
- **Also on the client:** focus, an open palette, sheet or dropdown, a list cursor, and an error from something just tried. `dsk.toml` is that client's configuration rather than state, so a laptop and a desktop can have different keys.

### Client engine (`engine`)

A Rust library embedded in both apps, holding the logic they share. The UI sends it commands and subscribes to state, and never touches a socket. (Settled)

- **In the engine:** the daemon list, one connection per daemon with reconnects, protocol types and version negotiation, event resume, the merged device and workspace tree, pending approvals across daemons, the local cache of last known state, and sending actions. The disk cache and `suspend()`/`resume()` aren't built yet. Last known state is kept in memory for now. Per-frame batching currently lives in the desktop app's bridge, not the engine.
- **In each app:** rendering, layout, scroll position, secure key storage, push registration, and telling the engine about lifecycle changes.
- **Streaming:** the engine batches updates roughly once per frame and sends small deltas, at both the Tauri IPC boundary and the Swift bindings.

### Desktop app

Tauri with React and Base UI, in `apps/dsk`. Its Rust side uses `engine` directly, with no FFI. (Settled) See `design-system.md`.

It's a first version on Tauri 2 (Tauri 3 is still in alpha), with the same features as the CLI and a layout that's expected to change. The bundle ID `dev.tectr.dsk` is a placeholder.

- **Bridge (Built):** `src-tauri/src/main.rs` has one pass-through `request` command for actions, plus `devices`, `open_session`, and `events_after` for reads. It fires `engine-changed` at most once per frame, and the webview rereads what it needs after the last sequence number it has, so a reload or an overlapping read can't lose events. Session events aren't pushed over IPC.
- **Permissions (Built):** the webview renders agent output, so it has a strict content security policy, and its only core permission is listening for events. It never receives raw HTML from an agent.
- **Daemons (Built):** the list comes from `TECTR_DAEMONS`, or else the local default. There's no UI for adding one, because daemons only listen on loopback until pairing exists. The app doesn't start a daemon itself.
- **Markdown (Built):** `apps/dsk/src/markdown.ts` compiles agent messages with `react-markdown` and `remark-gfm`. Agents write what's in it, so images aren't fetched, and links never navigate the webview. Web links go through the `open_url` command, which only accepts `http` and `https`, and other links are plain text. A streaming message is reparsed whole on each update, not incrementally as planned.
- **Transcript activity (Built):** `foldTranscript` in `apps/dsk/src/model.ts` folds the tool calls, thinking and answered requests between two messages into one activity, shown as a single line (`ActivityLine` in `design/spec/components.md`). Messages, errors and stop notes always stay visible. Durations come from the events' `at_ms`, so they survive a reload. The key `pane.toggle_steps` writes `appearance.transcript.steps` rather than holding view state of its own, so the file and Settings stay equal.
- **Streaming text (Built):** a message types out a letter at a time, in `useTypedLength` in `apps/dsk/src/Activity.tsx`. It compiles what's typed so far, so markdown takes shape as it goes. The rate follows what's left to show (`apps/dsk/src/motion.ts`): one letter per frame when it's close, faster when it's behind, so it settles about half a second behind a fast agent instead of falling further back. Text already there when a transcript opens never types: the daemon replays history an event at a time across many updates, so `transcriptTiming` in `model.ts` compares each event's `at_ms` with when the pane opened, and the first newer event is what starts typing and eased scrolling. This compares the daemon's clock with the client's, which is the same machine until pairing exists. With a remote device, a clock running ahead would type up to that much history. (Proposed: a replay-finished message from the daemon would replace the comparison.) This reparses the message every frame while typing, not just per update.
- **Scrolling (Built):** following new text and the scroll keys ease into place in about `motion.normal`, through one `newScroller` per transcript, driven by a `ResizeObserver` because a message keeps growing after its event arrived. Opening a transcript jumps to its end, and wheel, trackpad and scrollbar stay native and interrupt the easing. Only moving up stops following, because a table or code block can put the text far ahead of the scroll position between two frames. `appearance.transcript.animate` and the system's Reduce Motion turn typing and easing off.
- **Syntax highlighting (Built):** `crates/highlight` splits code into spans labelled by role, using `syntect` with the grammars from `two-face`. The `highlight_code` command runs it on a blocking thread, one request per block at a time, so text that's still streaming stays plain until the colors catch up. Fenced code with a known language gets colors. Inline code doesn't.
- **Typing a path on a device (Built):** the new workspace sheet suggests folders from the device it's adding to, through the daemon's `list_folders`, because the home folder and the disk belong to that machine. The daemon is also what expands `~`. Each folder is asked about once and filtered in the webview (`apps/dsk/src/pathInput.ts`), so typing within a folder never waits for a device. Going into a new folder is immediate on a loopback device and waits `appearance.layout.remote_path_delay` on any other. Enter completes the highlighted row, and with nothing highlighted it adds what's typed. A row that's already a workspace goes straight to it instead, landing on the topic last open there or else its newest, and a workspace with no topics lands on the new topic screen. Listings stop at 1000 folders, in name order, and a folder past that can still be typed. On macOS, listing the home folder doesn't check Desktop, Documents and Downloads for `.git`, because macOS would ask for permission first, so they're never tagged as repos at that level. Other protected places, like removable volumes, aren't special-cased.
- **Not built yet:** the virtualized transcript, tool details and diffs, the disk cache, and folding.

Commands and events between Rust and the webview should be type-generated, using `tauri-specta` or `ts-rs` if `tauri-specta` isn't mature enough. (Proposed; it was pre-1.0 when last looked at, and this hasn't been rechecked.) For now `apps/dsk/src/engine.ts` mirrors the protocol types by hand, so a wire change has to be made there too.

### iOS app

SwiftUI on iOS 26 and later, calling `engine` through Swift bindings generated by UniFFI. (Settled) See `design-system.md`.

It's an empty shell for now, in `apps/ios`. Only the `Tectr` target exists, and `TectrCore`, `TectrDesign`, and `TectrNotifications` get added when something needs them. `Tectr.xcodeproj` is generated from `project.yml` and stays out of git, so it needs `xcodegen generate` after a fresh clone. The bundle ID `dev.tectr.ios` is a placeholder.

iOS doesn't keep sockets open in the background, so the phone connects only while open and relies on push otherwise. The app calls `suspend()` and `resume()` on lifecycle changes, and every resume is a normal reconnect from the last sequence number. (Settled)

### Push relay (`push-relay`)

One small stateless service run by the publisher, because APNs credentials are tied to the publisher's developer account and can't ship in the daemon. Users host nothing. Daemons encrypt payloads so the relay never sees their contents, and if it's down only notifications are lost. (Settled)

The notification extension is a separate process with tight memory limits, so decryption can't load the full engine. (Settled) One crate, `push`, should own both encryption and decryption so the format lives in one place. (Proposed)

## Protocol and sync

| Decision | Status | Why |
|---|---|---|
| Websocket with tectr's own event schema, in a Rust crate shared by daemon and client | Settled | Clients stay independent of agent details |
| Version negotiated in the first handshake | Settled | Daemons on different machines can run different versions |
| Each session's events have a sequence number, and clients resume "after N" | Settled | Reconnecting is cheap, including every time the phone returns to the foreground |
| The device and workspace tree is sent as a snapshot on connect, followed by live changes | Proposed | Session sequence numbers don't cover topics and worktrees. The tree is small, so replaying its history isn't worth it |
| The protocol version is an explicit number, separate from crate versions | Proposed | Components ship independently |
| Serialized event fixtures are checked in | Proposed | A breaking wire change fails a test instead of breaking an older client |
| Terminals and diff comments arrive later as new message types | Proposed | Version negotiation already allows this, so they need no work now |
| A response arrives after every state change and followed event its command caused | Built | A client's state is current as soon as a command returns, so UIs can't show a stale status |
| Every enum that can grow has an `Unknown` variant | Built | An older client skips what it doesn't understand instead of failing |
| A client that falls behind gets a fresh snapshot and a refill from the database | Built | A slow client can't hold up the daemon or lose events |
| Prompts are refused while a turn is running, not queued | Built, can change | The simplest honest behavior for now. The Claude adapter reports support for queueing and steering, but other agents may not |

Client behavior that follows from this:

- **Instant open:** the client caches each daemon's last known state, renders it immediately, and catches up in the background.
- **Offline machines:** shown as offline with when they were last seen. Nothing new can be created on them.

## Agent integration (ACP)

The daemon is an ACP client and talks to agent adapters over JSON-RPC on stdio. Apps and `engine` never see ACP. (Settled)

```
engine ⇄ tectr event schema (websocket) ⇄ daemon ⇄ ACP (stdio) ⇄ adapter
```

| Decision | Status | Why |
|---|---|---|
| ACP types stay out of `protocol`; the daemon translates them | Proposed | ACP has no machine IDs or sequence numbers, and an ACP upgrade shouldn't force a client update |
| One adapter process per session | Proposed | A crash takes down one session, and interrupting and cleanup are simpler |
| The daemon installs pinned adapter versions into its own data directory | Proposed | The Claude adapter changes quickly, so an upstream release shouldn't break running setups. Rules out `npx` with the latest version |
| The daemon checks for Node at startup and reports a clear error | Proposed | See below |
| Core features don't rely on the SDK's `unstable_*` flags, and archiving is implemented in the daemon | Proposed | Session archive, fork, and status are still unstable |
| Each session reports what its agent can do, and the UI adapts | Proposed | Agents won't support the same features |

**Consequence:** the Claude adapter needs Node 22+, so the daemon can't be self-contained while it depends on that adapter. Node is required on every daemon machine, including a VPS.

Packages as of 2026-11-05. The Rust SDK and the Claude adapter are in use and tested with real prompts. The Codex adapter and `opencode acp` have been started and given a model, effort, and mode, but haven't run a prompt through tectr yet.

| Package | Version | Notes |
|---|---|---|
| `agent-client-protocol` (Rust SDK) | 5.0.0 | Needs Rust 1.88+; optional `process` and `stdio` features |
| `@agentclientprotocol/claude-agent-acp` | 0.100.0 | Needs Node 22+ |
| `@zed-industries/codex-acp` | 0.16.0 | Codex adapter. Uses the machine's Codex login |
| `opencode acp` | Whatever is installed | Built into OpenCode, so it isn't pinned |
| `@zed-industries/claude-code-acp` | 0.16.2 | Deprecated old name; don't use |

### What the spike found

Tested on 2026-11-04 against `claude-agent-acp` 0.100.0, with real prompts on Haiku.

- **Modes:** the adapter starts in `auto` and offers `default`, `acceptEdits`, `plan`, `auto`, and `bypassPermissions`. tectr maps manual to `default`, auto to `auto`, and yolo to `bypassPermissions`. If an agent can't honor the chosen mode, the session fails instead of running with different permissions.
- **Other agents' modes:** Codex maps manual to `read-only` ("Approval is required to edit files"), auto to `auto`, and yolo to `full-access`. OpenCode's modes are `build` and `plan` with no permission levels, so only auto is offered, as `build`, where OpenCode's own config decides what needs asking.
- **Changing the model can change other settings:** on Claude, selecting Haiku 4.5 moved the mode from `auto` to `acceptEdits`. So the daemon applies the model first, then checks and sets the mode, then sets effort. Effort levels depend on the model, and some models have none, such as Haiku 4.5 and Codex's `gpt-6-astra`.
- **Asking what an agent offers leaves a trace:** Claude creates an empty folder for the probe's working directory under `~/.claude/projects/`, and OpenCode adds an empty session to its history. Codex's session folder stayed empty.
- **Changing settings while a session runs (Built, tested only against `fake-agent`):** `set_session_settings` carries the model, effort, and mode together. A running agent gets them in the same order as at startup, even during a turn, because the Claude adapter queues configuration changes rather than refusing them. The session only changes once the agent has, so what's shown is what runs. If a change fails before anything moved, the agent carries on as it was. If it fails partway, or the agent doesn't answer within 15 seconds, the agent is stopped, because it could be running with permissions nobody chose. The next prompt starts it again with the session's settings. A stopped agent has nothing to refuse, so its settings are stored and used when it next starts.
- **`manual` doesn't mean every tool:** the adapter describes it as "always ask before making changes". A `Write` asked first, but `sleep 120` ran without asking, because Claude Code treats some commands as safe. Don't promise users that manual mode asks about everything.
- **Resume:** `session/resume` works from a new process, and the agent keeps its memory of the conversation. The daemon stores the agent's session ID and resumes when a prompt reaches a stopped or failed session. It sets the mode and model again each time. Other agents may not support this, so the daemon checks the agent's capabilities first.
- **Claude's own history:** Claude Code stores each conversation under `~/.claude/projects/`, keyed by working directory, and not in tectr's data directory. So tectr sessions appear in the user's Claude history, and that history stays when a topic's worktree is removed. Nothing in tectr can resume those sessions after that, because deleting a topic also removes its tabs.
- **Async runtime:** the SDK runs fine under Tokio. Its callbacks run in the connection's dispatch loop, so the daemon stores a permission responder and returns instead of waiting. A connection doesn't report a clean agent exit as an error, so the daemon has to watch `incoming_closed()` itself.
- **Cancel:** cancelling answers open permission requests, as ACP requires, and stopped a running `sleep` within about a second. A tool the model starts in the background can outlive the turn, because the adapter leaves background commands running.
- **Authentication:** `authMethods` is empty and the adapter silently uses the machine's Claude Code login. On a machine logged in with a subscription, tectr therefore drives Claude Code with that subscription, which is exactly the terms question listed under open questions. Answer it before anyone else uses this.
- **Not forwarded yet:** plans, usage, slash commands, and config changes are dropped. Agent questions other than permission requests aren't handled, so answering them is still to build.
- **Pinning is partial:** the daemon pins the adapter's own version, but npm can still pick newer versions of the adapter's dependencies. A checked-in lockfile would close this.

## Repository

A monorepo. (Settled) The layout below is Proposed.

```
tectr/
├── Cargo.toml              # workspace, shared deps and lints
├── Cargo.lock
├── rust-toolchain.toml     # Rust version and iOS targets
├── pnpm-workspace.yaml
├── mise.toml               # tool versions and tasks
├── crates/
│   ├── protocol/           # wire types, version negotiation, golden fixtures
│   ├── engine/             # shared by both apps: connections, merged state, actions
│   ├── highlight/          # syntax highlighting: code in, spans labelled by role out
│   ├── settings/           # the desktop app's settings file: format, defaults, themes, safe writes
│   ├── daemon/             # lib + bin; ACP stays inside here
│   ├── cli/                # minimal client for the first milestone
│   ├── ffi/                # UniFFI facade over engine
│   ├── push/               # payload encrypt/decrypt, no async runtime
│   ├── push-ffi/           # UniFFI facade for the notification extension
│   ├── push-relay/         # stateless APNs relay
│   └── fake-agent/         # scripted ACP agent for tests
├── apps/
│   ├── dsk/                # the desktop app
│   │   ├── src/            # web UI
│   │   └── src-tauri/      # workspace member, uses engine directly
│   └── ios/
│       ├── project.yml     # XcodeGen
│       ├── Tectr/
│       ├── TectrNotifications/
│       ├── TectrDesign/    # local Swift package: generated tokens + components
│       └── TectrCore/      # local Swift package wrapping the xcframeworks
├── design/                 # shared tokens, icons, specs, fixtures
├── context/
└── docs/
```

Create crates only when they're needed. See the build order below.

### Naming (Settled)

Crates have short names without a `tectr` prefix, because none of them are published to crates.io. Names that leave the repo keep it:

- **Binaries:** `tectr` (from `cli`) and `tectr-daemon` (from `daemon`), because they end up on your PATH.
- **The iOS library from `ffi`:** set its library name to `tectr_ffi`. Otherwise it builds `libffi`, which clashes with a widely used system library.

`engine` was called `tectr-client` at first. It's not an app: both apps embed it.

### Dependency rules (Proposed)

```
protocol ◄── daemon ──► push ◄── push-ffi
   ▲
engine ◄── cli, ffi, src-tauri
settings ◄── src-tauri
```

- `protocol` has no ACP types and no sockets.
- The daemon and engine never depend on each other in normal code. They meet only in end-to-end tests, which use `fake-agent` so CI needs no Claude credentials.
- `push` has no async runtime, and CI fails if `tokio` appears in its dependency tree.
- Swift only sees the FFI facades, so Swift needs no protocol codegen.

### Tooling

| Concern | Choice | Status | Why |
|---|---|---|---|
| Tool versions and tasks | mise, for both | Settled | One tool and config file. Tasks get pinned tools, and `sources`/`outputs` can skip the slow xcframework build. Replaces the earlier idea of adding `just` |
| Rust version | `rust-toolchain.toml`, not mise | Proposed | `rustup` and anything calling Cargo read it, and it can list iOS targets |
| Rust workspace | Cargo workspace with `[workspace.dependencies]` and `[workspace.lints]` | Proposed | One lockfile and one target directory |
| Web UI packages | pnpm | Proposed | Well supported by Tauri |
| Monorepo orchestrator | None (no Turborepo, Nx, Moon, or Bazel) | Proposed | One JS app, Cargo handles incremental Rust builds, and only two steps cross languages. Revisit if JS packages multiply or CI time becomes a problem |
| iOS project | XcodeGen | Proposed | Avoids `.pbxproj` merge conflicts |
| iOS bindings | Local `uniffi-bindgen` binary and a mise task that builds the xcframeworks; generated files stay out of git | Proposed | Keeps the bindgen version in step with the library |
| CI | GitHub Actions with path filters, `rust-cache`, `cargo-deny`, `mise-action` | Proposed | The expensive macOS iOS job runs only when `apps/ios`, FFI, or their dependencies change |
| Releases | A separate tag per component, such as `daemon-v0.3.0` and `ios-v1.2.0` | Proposed | Components ship at different times |

### Known pitfalls

- **`default-members`:** leave `src-tauri` out. Otherwise a plain `cargo build` needs the built frontend and, on Linux, WebKit, which would also break daemon builds on a VPS. `--workspace` overrides this and includes the desktop app, so use plain `cargo test` and `cargo clippy` for everything else.
- **Notification extension size:** build `push-ffi` on its own with `-p` and a size-optimized profile, because `--workspace` builds can pull in extra features.
- **Duplicate symbols:** link each iOS target to only one FFI library, because linking both can cause duplicate Rust symbols.
- **Xcode build phases:** they don't get the shell's mise setup. Use the full path to `mise`, or run `mise run ios:core` from the terminal first.

## Performance

(Settled)

- The transcript is virtualized, so only visible messages are rendered.
- Markdown is parsed incrementally as tokens stream in, and updates are batched per frame.
- Syntax highlighting and diffing run off the UI thread.
- The cached state makes opening the app instant, even when daemons are remote.

## Why no central server

Routing everything through a server would make local use depend on a remote machine. An outage would cut you off from agents on your own laptop, a local app would require hosting a server first, and the server would see all code and transcripts and could approve tool calls on every machine. (Settled)

## Build order

### First milestone

Built. `idea.md` still has the original three steps. This is the version that was built, updated for the workspace and topic model:

A daemon that can:

1. Add an existing repo as a workspace and create a topic with a fresh worktree, persisted across restarts.
2. Start a Claude Code session through ACP in that worktree.
3. Stream its events and relay a permission request.
4. Accept a prompt from a minimal client.

Requirements from the start, because they're expensive to add later:

- Machine-scoped IDs. (Settled)
- Resumable events. (Settled)
- A client built around a list of daemons. (Settled)
- Version negotiation. (Settled)
- A tree snapshot on connect, followed by live changes. (Proposed)

To prove step 3 reliably, `fake-agent` triggers a permission request on demand, and the manual test against Claude uses a mode that prompts. In auto mode a real agent may rarely ask, so the step could look finished without being exercised.

The end-to-end tests live in `crates/fake-agent/tests/`, because Cargo only gives a test the path of a binary in its own package.

Crates needed: `protocol`, `daemon`, `engine`, `cli`, `fake-agent`.

Not in this milestone:

- Pairing, the push relay, and Tailscale discovery. (Settled)
- Tabs, terminals, diff comments, and adding workspaces from a git URL or a new repo. (Proposed)
- The twig decision, because plain `git worktree add` is enough. (Proposed)

### After that

(Proposed, except where noted)

1. `ffi`, and the empty iOS app showing the session list, which only tests the iOS build and bindings. (Settled that this comes soon after the milestone)
2. Pairing and Tailscale discovery, before the daemon listens beyond loopback.
3. `push`, `push-ffi`, and `push-relay`.

## Open questions

From `idea.md`. The terms question now matters in practice, because the adapter uses the machine's Claude login by default:

- **Auth and terms:** what do Anthropic's terms allow when a third-party client drives Claude Code with a claude.ai subscription rather than an API key?
- **Relationship to twig:** should the daemon call twig, which is written in Go, as a separate process, or reimplement its worktree logic? Worktree logic can start as a daemon module either way.
- **Agents sharing a worktree:** can two agents run at once in one worktree, or only one at a time?
- **Finished topics:** are they archived and their worktrees removed, or kept in a folded "Done" group?
- **Desktop and phone layout:** the desktop UX isn't settled, and the phone app hasn't been redesigned around workspaces and topics.

Raised during design:

- **Permission modes:** does auto mode still send some requests to the user, with only yolo never asking? `idea.md` line 16 no longer mentions approving tool calls. The working assumption is yes, and the daemon relays whatever the agent asks in any mode. How often Claude's auto mode asks hasn't been measured. It's described as "Claude handles permission decisions".
- **Draft diff comments:** do they live on the daemon or the client? This decides whether a review started on desktop can be continued on the phone.

## Possible later additions

- **Relay for people without Tailscale:** an optional relay that only forwards end-to-end encrypted traffic.
- **Hub for team views or a web client:** an optional aggregator that connects to daemons as a client and offers the same protocol to apps, which treat it as just another daemon.
