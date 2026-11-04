# tectr: architecture and design decisions

Last updated: 2026-11-04. The first milestone is built: `protocol`, `daemon`, `engine`, `cli`, and `fake-agent`. Everything else here is still a design, not a description of a working system.

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
| Tabs are not a daemon concept | Proposed | Layout belongs to each app. A tab is a client view of a session, a terminal, or a diff |
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

### Client engine (`engine`)

A Rust library embedded in both apps, holding the logic they share. The UI sends it commands and subscribes to state, and never touches a socket. (Settled)

- **In the engine:** the daemon list, one connection per daemon with reconnects, protocol types and version negotiation, event resume, the merged device and workspace tree, the approvals inbox, the local cache of last known state, and sending actions. The disk cache, per-frame batching, and `suspend()`/`resume()` aren't built yet, because nothing uses them until the first real UI. Last known state is kept in memory for now.
- **In each app:** rendering, layout, scroll position, secure key storage, push registration, and telling the engine about lifecycle changes.
- **Streaming:** the engine batches updates roughly once per frame and sends small deltas, at both the Tauri IPC boundary and the Swift bindings.

### Desktop app

Tauri with React and Base UI, in `apps/dsk`. Its Rust side uses `engine` directly, with no FFI. (Settled) See `design-system.md`.

It's an empty shell for now, on Tauri 2 (Tauri 3 is still in alpha), with no dependency on `engine`. Base UI isn't installed until the first component uses it. The webview has no IPC permissions and a strict content security policy, which matters once it renders agent output. The bundle ID `dev.tectr.dsk` is a placeholder.

Commands and events between Rust and the webview should be type-generated, using `tauri-specta` or `ts-rs` if `tauri-specta` isn't mature enough. (Proposed; it was pre-1.0 when last looked at, and this hasn't been rechecked.)

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

Packages as of 2026-11-04. The Rust SDK and the Claude adapter are in use and tested. The other two haven't been run.

| Package | Version | Notes |
|---|---|---|
| `agent-client-protocol` (Rust SDK) | 5.0.0 | Needs Rust 1.88+; optional `process` and `stdio` features |
| `@agentclientprotocol/claude-agent-acp` | 0.100.0 | Needs Node 22+ |
| `@zed-industries/codex-acp` | 0.16.0 | Codex adapter |
| `@zed-industries/claude-code-acp` | 0.16.2 | Deprecated old name; don't use |

### What the spike found

Tested on 2026-11-04 against `claude-agent-acp` 0.100.0, with real prompts on Haiku.

- **Modes:** the adapter starts in `auto` and offers `default`, `acceptEdits`, `plan`, `auto`, and `bypassPermissions`. tectr maps manual to `default`, auto to `auto`, and yolo to `bypassPermissions`. If an agent can't honor the chosen mode, the session fails instead of running with different permissions.
- **`manual` doesn't mean every tool:** the adapter describes it as "always ask before making changes". A `Write` asked first, but `sleep 120` ran without asking, because Claude Code treats some commands as safe. Don't promise users that manual mode asks about everything.
- **Resume:** `session/resume` works from a new process, and the agent keeps its memory of the conversation. The daemon stores the agent's session ID and resumes when a prompt reaches a stopped or failed session. It sets the mode and model again each time. Other agents may not support this, so the daemon checks the agent's capabilities first.
- **Claude's own history:** Claude Code stores each conversation under `~/.claude/projects/`, keyed by working directory, and not in tectr's data directory. So tectr sessions appear in the user's Claude history, and removing a worktree may affect resuming its sessions. Decide this when building worktree cleanup.
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
2. Connect `apps/dsk` to `engine`.
3. Pairing and Tailscale discovery, before the daemon listens beyond loopback.
4. `push`, `push-ffi`, and `push-relay`.

## Open questions

From `idea.md`. The terms question now matters in practice, because the adapter uses the machine's Claude login by default:

- **Auth and terms:** what do Anthropic's terms allow when a third-party client drives Claude Code with a claude.ai subscription rather than an API key?
- **Relationship to twig:** should the daemon call twig, which is written in Go, as a separate process, or reimplement its worktree logic? Worktree logic can start as a daemon module either way.
- **Agents sharing a worktree:** can two agents run at once in one worktree, or only one at a time?
- **Finished topics:** are they archived and their worktrees removed, or kept in a folded "Done" group?
- **Desktop and phone layout:** the desktop UX isn't settled, and the phone app hasn't been redesigned around workspaces and topics.

Raised during design:

- **Permission modes:** does auto mode still send some requests to the user, with only yolo never asking? `idea.md` line 16 no longer mentions approving tool calls, but line 28 keeps the approvals inbox. The working assumption is yes, and the daemon relays whatever the agent asks in any mode. How often Claude's auto mode asks hasn't been measured. It's described as "Claude handles permission decisions". If it rarely asks, the inbox matters less than planned.
- **Draft diff comments:** do they live on the daemon or the client? This decides whether a review started on desktop can be continued on the phone.

## Possible later additions

- **Relay for people without Tailscale:** an optional relay that only forwards end-to-end encrypted traffic.
- **Hub for team views or a web client:** an optional aggregator that connects to daemons as a client and offers the same protocol to apps, which treat it as just another daemon.
