# tectr: architecture and design decisions

Last updated: 2026-11-10. The first milestone is built: `protocol`, `daemon`, `engine`, `cli`, and `fake-agent`. `apps/dsk` is connected to `engine` and matches the CLI's features. Everything else here is still a design, not a description of a working system.

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
    │ loopback          │ │ iroh       │    websocket (tectr event schema)
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
    └── Topic        a feature or task, working in one folder at a time
        ├── Worktree (many)   folders it has worked in, the repo's own included
        ├── Session (many)    an agent run, in the topic's folder
        └── Terminal (many)
```

| Decision | Status | Why |
|---|---|---|
| A workspace belongs to one device, so the same repo on two devices is two workspaces | Settled | Each daemon stays the single source of truth for its own work |
| A topic works in one folder at a time, a worktree or the repo's own, and can move between them | Settled | A task's tabs can't drift apart, and a discussion at the repo root can carry on as the implementation in a worktree. Branches inside a folder are git's business, so one topic can still end in several PRs. Work that has to run side by side in isolation is a second topic |
| The daemon owns all state, including tabs, and each client has its own view of it | Settled | Every client sees the same topics, tabs, agents and drafts, and moving around on one doesn't move the others. See Shared state and separate views |
| A topic's tabs all work in its folder at once | Proposed | They're one task, so they share its files. SQLite can't drop the old per-session `worktree_id` column, which is kept equal to the topic's folder |
| Agent and permission mode are stored per session | Proposed | The agent is chosen per tab, and sessions can run in different modes |
| IDs are scoped by machine, such as `(machine_id, session_id)` | Settled | Clients merge many daemons without collisions |

## Components

### Daemon (`daemon`)

Runs on each machine. It owns workspaces, topics, sessions, and terminals, spawns agents in their worktrees, and exposes one stable event schema over a websocket.

- **Source of truth (Settled):** clients can come and go without losing anything.
- **Persistence in SQLite from day one (Proposed):** workspaces, topics, and session events must survive a daemon restart. Resume also depends on stored events.
- **Loopback or a paired key (Built):** the daemon can drive agents and runs shells for terminals, so an unauthenticated network listener is effectively remote code execution. The websocket's address is hardcoded to loopback, so no setting can change it. The MCP proxy and login redirects share that listener (see MCP servers). Remote clients come in only over iroh, with a key on the allowed list (see Remote connections).
- **Browsers are rejected (Built):** any web page can open a websocket to loopback, so the daemon refuses any connection with an `Origin` header. Native clients don't send one.
- **Other users on the same machine aren't blocked yet (Known gap):** loopback has no authentication, so on a shared machine another local user could connect and run agents as you. Don't run the daemon on a shared machine until loopback clients also prove a key. Such a user could also start a pairing and let in a machine of theirs.
- **Dev and release channels (Built):** a build is dev unless `scripts/release.sh` builds it with `TECTR_CHANNEL=release` (`protocol::Channel`). Release uses port 7767, `<data dir>/tectr` and `~/.config/tectr`, and dev uses 7769, `tectr-dev` and `~/.config/tectr-dev`, so working on tectr never touches the installed daemon's state. Defaulting to dev means a stray `cargo build --release` can't take over the installed daemon either.
- **Installed as a LaunchAgent (Built):** the Homebrew cask registers `dev.tectr.daemon`, so the released daemon starts at login, restarts after a crash, outlives the app, and restarts onto the new binary on `brew upgrade`, which stops running agents until their next prompt. Logs go to `~/Library/Logs/tectr/daemon.log`. It stops on SIGTERM the same way as on Ctrl-C.
- **The login shell's environment (Built):** launchd starts programs with a bare `PATH`, which wouldn't find agents installed through nvm or into `~/.local/bin`. With `--shell-env`, which the LaunchAgent passes, the daemon reads the environment of an interactive login shell, with a 10 second limit, and starts itself over in it, so agents see what they would in a terminal. Changing its own environment instead isn't safe once threads run. If the shell can't be read, it carries on with what it has. A changed shell setup applies on the next restart.
- **Crash recovery (Built):** agent processes don't survive the daemon. On startup, sessions that were starting, working, or waiting become `stopped`, and open approvals are resolved as cancelled.
- **Worktrees live in the data directory (Proposed):** `<data dir>/worktrees/<workspace>/<slug>-<id>`, on a new branch `tectr/<slug>-<id>` from the repo's `HEAD`. This keeps the user's repo folder clean.
- **Topics at the repo root (Proposed):** a topic can work in the repo's own folder instead, and the desktop new topic form starts on this choice. It's stored as a worktree record pointing at the workspace path, so sessions need no special case. Archiving never touches it.
- **Device settings (Settled):** settings specific to a device live in its daemon, in SQLite, so every client sees the same ones. Client settings stay on the client (see Desktop settings). Providers are the first device setting.
- **Providers (Proposed):** the daemon knows how to run Claude Code, Codex, and OpenCode, plus anything passed with `--agent`. Checking which are available only looks on `PATH` and starts nothing. An agent starts only once it's added to that device. It starts in an empty folder with no prompt, and the daemon stores the modes, models, and per-model effort levels it reports. Nothing is hardcoded, so new models need no app update. Added providers are asked again each time the daemon starts, and one that fails keeps what it last reported. OpenCode is the exception and only updates on Add or Refresh, because each time it's asked it adds an empty "New session" to the person's OpenCode history. Each provider has a default model and effort, chosen in the device's settings and kept when it's asked again. "Default" entries from agents are never offered, so what's shown is what runs. Only a model the agent itself names as recommended is marked, as Claude does. No agent says which effort it recommends, so none is marked. A provider is only a list for pickers: `start_session` doesn't require one, so the CLI and `--agent` setups keep working.
- **Keep awake (Built):** a device setting, on by default, that stops the machine idle sleeping while any of its agents is starting or working, so locking the screen doesn't freeze a turn halfway. Waiting for an answer doesn't count, because nothing moves until someone is back. On macOS the daemon runs `caffeinate -i -w <daemon pid>` for exactly that time, instead of calling IOKit, which would need `unsafe`. `-w` ends it with the daemon even if the daemon is killed. It only holds off idle sleep, so closing a laptop's lid still sleeps. Other platforms sleep as usual. The settings are one JSON value in `meta`, so a new one needs no migration.
- **Topic names (Proposed):** a topic created without a name is numbered within its workspace, like "Topic #3", and goes by its first open tab's title once that has one, following later changes to it. The daemon does this, so every client sees the same name. Choosing a name, on creation or with `rename_topic`, stops it for good, and the branch and worktree keep the name they were made with.
- **Archiving a topic (Proposed):** `archive_topic` stops the topic's agents and takes it out of the app, keeping its tabs and their events in the database. Its worktrees always go, forced, along with the branches tectr made for them, once the agents have exited, so the command returns straight away. A branch an agent made itself stays, and so does the repo's own folder. Without `force`, the daemon first asks git what that would lose and does nothing if anything would, answering with what: running agents, uncommitted files per worktree, and commits on a branch tectr made that no other branch or remote has. A pushed branch with an open PR isn't at risk. The desktop app archives straight away when nothing is, asks once when only agents would stop, and asks twice when work in git would go, the second time listing exactly what. Those confirmations take `sheet.confirm`, `y` by default, and Cancel is the focused primary button, so an Enter out of habit cancels. `d d` archives, and "Restore an archived topic" in the palette lists them across devices, newest first. Restoring brings the tabs back and makes the topic's folder again at the same path, on its branch if that still exists locally or on a remote and otherwise on a new one from `HEAD`, so agents resume where they were. Only the folder its tabs were in comes back. A daemon-wide lock keeps a restore from racing the removal of the same folder. Archived topics are never deleted for good yet.
- **Moving a topic (Proposed):** `move_topic` takes all of a topic's tabs to a fresh worktree, branched from the commit the topic is on, to the repo's own folder, or back to a worktree it left. The folder left behind stays until the topic is archived. Agents stop, which a running one needs confirming for, and resume in the new folder on their next prompt. That prompt starts with a line telling the agent where it now works, because its history is full of paths in the old folder that it would otherwise keep editing. The daemon remembers the folder each agent last heard about, so the line comes once per move. Uncommitted changes don't come along.
- **Terminals (Built):** a terminal is a shell on a pseudo terminal in the daemon (`crates/daemon/src/terminal.rs`), started as the person's `$SHELL -l` in the topic's folder with `TERM=xterm-256color`. Each has a thread that owns a copy of the terminal in Ghostty's engine, libghostty-vt, which can't move between threads, plus one that reads the shell and one that writes to it, so a program that stops reading can't stall its output. A client attaching gets a reset with everything the terminal shows, modes and cursor included, from Ghostty's formatter, then what's printed. The palette is left out of it, so each client keeps its theme's colors. Clients acknowledge what they've drawn. One more than 4 MiB behind is sent nothing until it's back under 512 KiB, and then the screen as it is by then, so a flood of output never queues anywhere and a slow client never slows the shell. Ghostty answers programs' questions, such as where the cursor is, even with no client showing the terminal, and clients leave them alone so there's one answer. It's never told about the kitty keyboard protocol, because xterm.js, which turns keys into bytes, doesn't speak it. The client in use sets the size: attaching, typing or resizing a window sends it, and every client showing the terminal starts over at that size. Tabs go in the order they were opened, agents and terminals together, so sessions and terminals both carry `opened_at_ms`.
- **Terminals across restarts (Built):** no shell outlives the daemon, but its tab does. A terminal saves the main screen's text and colors with its history to `<data dir>/terminals/<id>.vt` once output has been quiet for 2 seconds, at least every 30 seconds while it isn't, and when the daemon stops, as plain escape sequences, so a newer Ghostty can still read them. Where the shell is goes in the database, read with `lsof` on macOS because reading it directly would need `unsafe`. After a restart a terminal is stopped until a client shows it, and then starts a shell where the last one was, or in the topic's folder if that's gone, below what the last one left and a line. A program like an editor on the other screen ended with the shell, so what's under it is what comes back. Stopping sends the shell SIGHUP and doesn't wait, so it can write its own history. A shell that exits cleanly closes its tab, and one that fails stays to show why, and typing starts another. Archiving stops a topic's shells and keeps what they showed, moving a topic leaves them running where they are, and closing a terminal deletes its history.
- **Branches are read from git (Proposed):** a folder's branch is what it has checked out, read at startup, after every turn and when a topic moves or comes back, because agents switch branches. The database keeps the branch each worktree was made with, which is the one archiving deletes.

### Desktop settings (`settings`)

- **One file (Settled):** `~/.config/tectr/dsk.toml` holds every setting of the desktop app (`~/.config/tectr-dev/dsk.toml` for a dev build, so settings only dev knows don't show up as problems in the installed app), one section per area, currently `[keybinds]` and `[appearance]`. It's TOML because people and agents both edit it, so it needs comments, and `toml_edit` can change one value while keeping the rest as written.
- **Only what differs (Settled):** defaults live in the `settings` crate, so the file stays short enough for an agent to edit by line. `dsk.defaults.toml` next to it is rewritten on launch and lists every setting and its default. It's the reference, and is never read back.
- **Everything is configurable (Settled):** every key in the app is an action in `crates/settings/src/keybinds.rs`, including permission answers, arrows, and whether a key has to be held (`hold_ms`) or repeats (`repeat`). `apps/dsk/src/keybinds.ts` lists the same IDs, which a test checks. What's left to the system: editing keys inside text fields, Enter and Space on a focused button, and what the macOS menu takes first, which the file warns about.
- **Shared navigation with overrides (Settled):** `[keybinds.nav]` is the default for moving around. Actions follow it until they're set themselves, so changing `nav.down` moves everything except what was set on its own. Lists you search by typing, which today means the palette and pickers, follow `nav.input_up` and `nav.input_down` instead, because letters there have to reach the box. Those default to Ctrl+K and Ctrl+J on macOS. Elsewhere `mod` is Ctrl, and Ctrl+K toggles the palette, so they default to only the arrows.
- **Vim-inspired keys (Settled):** Vim's keys are the default wherever an action means the same thing there, such as `gg`, `G`, `zz`, `gt`, `dd`, `Ctrl+D`. What Vim has no word for keeps its ⌘ key, and the ⌘ and arrow keys stay alongside the Vim ones.
- **Sequences work like Vim's (Built):** a key that starts a sequence waits for the next until `keybinds.sequence_timeout_ms`, and a key that doesn't continue it drops what came before and acts on its own. Two differences: a key that also starts a longer sequence fires straight away rather than waiting, which the settings screen warns about, and keys that type can't be part of a sequence in a text box, so `jk` can't be a keybind there. Vim keys in text boxes have their own `jk` instead, as `appearance.vim.escape`.
- **Vim in text boxes (Built):** `appearance.vim.enabled`, off by default, gives the message box and the new topic box normal, insert and visual mode: motions with counts, `d`, `c` and `y` with motions and text objects, `p`, `u`, `r`, `J` and `~`. There are no macros, marks, named registers, ex commands or dot repeat, because a message box doesn't need them. `apps/dsk/src/vim.ts` is the engine and is pure, so its tests need no DOM and any editor can sit on it later. `apps/dsk/src/VimBox.tsx` puts it under a textarea, making every edit through the text box's own editing commands so that undo, React and the daemon's draft see ordinary edits, and `u` is the text box's own undo. Keys vim doesn't take, such as Enter, Tab and Cmd shortcuts, go on to the box's keybinds. Esc in insert mode goes to normal mode, and in normal mode does what it did before, so leaving the box takes two. Normal mode draws its block by selecting the character under the cursor. Going to a box from elsewhere in the app starts it in `appearance.vim.start`, but switching apps or opening a list from the box doesn't. `global.toggle_vim`, with no key by default, and the palette turn it on and off.
- **Vim and the system clipboard (Built):** with `appearance.vim.system_clipboard`, on by default, yanking and deleting also copy to the system clipboard and `p` puts what's there. Off, vim keeps its register to itself. WebKit asks before every clipboard read from a page, so it goes through Rust. `p` can't wait for a read, so the register is refreshed whenever the clipboard may have changed: coming back to the window, copying or cutting, and going to a box. Text that ends in a line break puts as whole lines, unless it's what vim yanked last, which keeps how it was yanked.
- **A bad file never costs the keyboard (Built):** a syntax error keeps the last good settings. A bad entry falls back to its default. Both are shown on the settings screen, which refuses to save over a file that doesn't parse.
- **The webview can't touch the file (Built):** Rust owns it and exposes only `settings`, `set_keybind`, `reset_keybind`, `set_appearance` and `reset_appearance`, each of which checks what it's given, because the webview renders agent output. `try_window_theme` is the one other thing it can do to the app's look, and it only names a theme that exists and saves nothing. Apart from the file, `read_clipboard` and `write_clipboard` let vim share the system clipboard. Agent output is rendered as Markdown without raw HTML, so it can't call them.
- **Appearance is rows from a table (Built):** `FIELDS` in `crates/settings/src/appearance.rs` holds each setting's default and what it can be. The screen draws its rows from that, so a new setting there needs only a line in `cssVariables` in `apps/dsk/src/appearance.ts`. Rust works out every theme's colors, and the webview only turns values into CSS variables on `<html>`. The native title bar is the one thing it can't draw, so Rust also sets the window's color and whether macOS draws for dark or light, including for a theme being tried out in a list.
- **Themes are data (Built):** themes that come with the app are in `crates/settings/src/themes.toml`, in the format of a theme in the settings file. A theme sets what it wants and the rest follows from rules, so changing an accent changes the tints made from it. Tectr's own two have to match `design/tokens`, which a test checks against `design/tokens/generated/desktop-defaults.json`.
- **A theme belongs to one mode (Settled):** there's one theme chosen for dark and one for light, and a list only offers themes of its mode. A light theme in `dark_theme` would leave the title bar's text unreadable, so the file falls back to the default and says so.
- **The shortcut line (Built):** `appearance.layout.show_shortcuts`, off by default, draws a line along the bottom of the window with the keys that work right now. It's a client setting because it's about this keyboard, and it has to work with no daemon reachable. Each screen declares its hints with `useHints` next to the handlers that run those keys, leaving out what would do nothing, and `chooseHints` in `apps/dsk/src/shortcuts.ts` picks by what has the keyboard: an overlay hides everything behind it, and otherwise it's the sidebar row, the pane or the text field, followed by the global keys. A hint names actions rather than keys, so it follows rebinding, and `formatKeys` shows actions that belong together as one label like `K/J` or a range like `Cmd 1-9`. What doesn't fit is dropped as whole hints from the end, except the pinned palette and next request keys.
- **A launch starts from what the last one showed (Built):** the webview keeps the last appearance in local storage and paints it before anything else, so a theme doesn't flash the default on the way in. It's a copy, so losing it costs one flash.

### Shared state and separate views (Settled)

The daemon owns what exists, and each client chooses what to look at. Every client sees the same topics, tabs, agents, permission requests and unsent drafts, and opening a topic on one doesn't move the others.

- **Which tabs exist is shared, and which one is showing isn't.** A new tab appears everywhere, and only the client that started it switches to it. If another client closes the tab you're on, you move to the next one, or else the previous one.
- **Unread is shared.** A session turns `unread` when a turn finishes, and `set_unread` changes it. It's about the person rather than a client, so reading a result on the phone reads it on the desktop too. Starting new work clears it.
- **Drafts are shared.** `DaemonState.drafts` is stored in SQLite and sent to every client, so a message started on one can be finished on another. The last write wins. The images a draft names are in `DaemonState.draft_images`, kept apart so the format of `drafts` stays what older clients read.
- **Closing a tab stops its agent.** `close_session` kills the process, removes the session from state and drops its draft, keeping its events in the database. It refuses a running agent without `force`, so the second confirmation holds for every client, not only the ones that ask.
- **Navigation stays on the client.** `apps/dsk/src/navigation.ts` holds the current screen, the selected tab in each topic, which topics have a new tab open, where going back leads, the last topic per workspace and the settings section. It's kept in the webview's local storage, so a restarted client opens where it was and a new one opens on the empty state. Losing it costs nothing but the place you were. It needs no daemon, so a client with none reachable can still open its own settings, and a view can name an offline device.
- **The new topic form stays on the client and isn't saved.** It lives in memory until the topic starts or the form is cancelled, and restarting clears it. Cancelling goes back to the topic that was open before it. With no topic to go back to, such as after landing in an empty workspace or coming back from settings, it goes to the sidebar instead and keeps the form.
- **A new tab is the client's until something starts from it.** `global.new_tab` opens a tab at the end of the strip that asks what to open: an agent, a terminal, git or a diff, by number. Agents and terminals are built. A terminal needs no setup, so choosing one opens it in the new tab's place. Choosing an agent shows the same form as the new topic screen, minus the workspace, and starting it replaces the new tab with the session, which every client then sees. Other clients never see the new tab itself, and what's typed in it lives in memory like the new topic form. Closing it drops what was typed. A topic with no sessions always shows one, since that's all there is to do there, so closing its last tab lands on it. Choosing in that tab keeps it open, so an agent another client starts there doesn't take it away mid-sentence.
- **Also on the client:** focus, an open palette, sheet or dropdown, a list cursor, and an error from something just tried. `dsk.toml` is that client's configuration rather than state, so a laptop and a desktop can have different keys.

### Client engine (`engine`)

A Rust library embedded in both apps, holding the logic they share. The UI sends it commands and subscribes to state, and never touches a socket. (Settled)

- **In the engine:** the daemon list, one connection per daemon with reconnects, protocol types and version negotiation, event resume, the merged device and workspace tree, pending approvals across daemons, the local cache of last known state, and sending actions. The disk cache and `suspend()`/`resume()` aren't built yet. Last known state is kept in memory for now. Per-frame batching currently lives in the desktop app's bridge, not the engine.
- **In each app:** rendering, layout, scroll position, secure key storage, push registration, and telling the engine about lifecycle changes.
- **Streaming:** the engine batches updates roughly once per frame and sends small deltas, at both the Tauri IPC boundary and the Swift bindings.
- **Terminals (Built):** the engine attaches the terminals a UI shows and attaches them again after a reconnect, which starts each with a reset. Their frames go straight to a sink the UI gives it, and typing and acknowledgements go straight out, as binary websocket frames. Both ends set `TCP_NODELAY`, so a keystroke's echo isn't held back to be combined with others.

### Desktop app

Tauri with React and Base UI, in `apps/dsk`. Its Rust side uses `engine` directly, with no FFI. (Settled) See `design-system.md`.

It's a first version on Tauri 2 (Tauri 3 is still in alpha), with the same features as the CLI and a layout that's expected to change. The bundle ID `dev.tectr.dsk` is a placeholder.

- **Bridge (Built):** `src-tauri/src/main.rs` has one pass-through `request` command for actions, plus `devices`, `open_session`, and `events_after` for reads. It fires `engine-changed` at most once per frame, and the webview rereads what it needs after the last sequence number it has, so a reload or an overlapping read can't lose events. Session events aren't pushed over IPC.
- **Permissions (Built):** the webview renders agent output, so it has a strict content security policy, and its only core permission is listening for events. It never receives raw HTML from an agent. Images may also come from `blob:` URLs, which only the webview's own code can make, for showing pasted and sent images.
- **Daemons (Built):** the list comes from `TECTR_DAEMONS`, or else the local daemon of the build's channel and every device paired from this machine. App settings' Devices section lists them. "Add a device" in the palette or there pastes a code, "Pair another machine" shows one for this machine's daemon, and a device's Pairing settings list the machines that can connect, with this app's own row on another device being how it stops reaching it. The app doesn't start a daemon itself: when installed, a LaunchAgent runs it, and in development `mise run daemon` does. A dev build titles its window "tectr dev".
- **Markdown (Built):** `apps/dsk/src/markdown.ts` compiles agent messages with `react-markdown` and `remark-gfm`. Agents write what's in it, so images aren't fetched, and links never navigate the webview. Web links go through the `open_url` command, which only accepts `http` and `https`, and other links are plain text. A streaming message is reparsed whole on each update, not incrementally as planned.
- **Transcript activity (Built):** `foldTranscript` in `apps/dsk/src/model.ts` folds the tool calls, thinking and answered requests between two messages into one activity, shown as a single line (`ActivityLine` in `design/spec/components.md`). Messages, errors and stop notes always stay visible. Durations come from the events' `at_ms`, so they survive a reload. The key `pane.toggle_steps` writes `appearance.transcript.steps` rather than holding view state of its own, so the file and Settings stay equal. Unfolding one activity or the current turn (`pane.toggle_turn`, or clicking the live line) is view state in the pane, in `foldState`, and resets when the setting changes.
- **Streaming text (Built):** a message types out a letter at a time, in `useTypedLength` in `apps/dsk/src/Activity.tsx`. It compiles what's typed so far, so markdown takes shape as it goes. The rate follows what's left to show (`apps/dsk/src/motion.ts`): one letter per frame when it's close, faster when it's behind, so it settles about half a second behind a fast agent instead of falling further back. Text already there when a transcript opens never types: the daemon replays history an event at a time across many updates, so `transcriptTiming` in `model.ts` compares each event's `at_ms` with when the pane opened, and the first newer event is what starts typing and eased scrolling. This compares the daemon's clock with the client's, so with a remote device a clock running ahead would type up to that much history. (Proposed: a replay-finished message from the daemon would replace the comparison.) This reparses the message every frame while typing, not just per update.
- **Scrolling (Built):** following new text and the scroll keys ease into place in about `motion.normal`, through one `newScroller` per transcript, driven by a `ResizeObserver` because a message keeps growing after its event arrived. Opening a transcript jumps to its end, and wheel, trackpad and scrollbar stay native and interrupt the easing. Only moving up stops following, because a table or code block can put the text far ahead of the scroll position between two frames. `appearance.transcript.animate` and the system's Reduce Motion turn typing and easing off.
- **Syntax highlighting (Built):** `crates/highlight` splits code into spans labelled by role, using `syntect` with the grammars from `two-face`. The `highlight_code` command runs it on a blocking thread, one request per block at a time, so text that's still streaming stays plain until the colors catch up. Fenced code with a known language gets colors. Inline code doesn't.
- **Typing a path on a device (Built):** the new workspace sheet suggests folders from the device it's adding to, through the daemon's `list_folders`, because the home folder and the disk belong to that machine. The daemon is also what expands `~`. Each folder is asked about once and filtered in the webview (`apps/dsk/src/pathInput.ts`), so typing within a folder never waits for a device. Going into a new folder is immediate on a loopback device and waits `appearance.layout.remote_path_delay` on any other. Enter completes the highlighted row, and with nothing highlighted it adds what's typed. A row that's already a workspace goes straight to it instead, landing on the topic last open there or else its newest, and a workspace with no topics lands on the new topic screen. Listings stop at 1000 folders, in name order, and a folder past that can still be typed. On macOS, listing the home folder doesn't check Desktop, Documents and Downloads for `.git`, because macOS would ask for permission first, so they're never tagged as repos at that level. Other protected places, like removable volumes, aren't special-cased.
- **Device settings sections (Built):** device settings has General, Providers, MCP servers and Pairing, and works like App settings: it opens on the list of sections, Enter or `l` goes into one, and `settings.back` comes back out to the list, with `sections.close` closing settings. Every settings list looks and moves the same way: grouped one-line rows, the `settings.*` keys to move, what can be done to the selected row in a bar at the bottom, so moving never shifts the list, and what applies to the whole list, like adding an MCP server, next to its group's heading. General holds keep awake and the name, which is edited right in its row, with the palette's rename landing there too. Providers lists the added ones, with their default model and effort on the row, above the available ones. Pairing lists the machines that can connect, with Pair next to its heading on this machine's own daemon. App settings has a Devices section too, listing every device this app reaches with Add a device and Pair another machine next to its heading, Enter opening a device's settings and `pairing.remove` taking a paired one out of the app. The open section is part of the view, so a palette entry can open settings straight at MCP servers.
- **MCP servers (Built):** the MCP servers section lists the device-wide servers with keys to add, edit, remove, turn each one's default on or off, log in, log out and import. With none yet, Import is the primary action, because bringing over what the agents had is most people's whole setup. `global.topic_mcp` opens a sheet for the current topic, which sets each device server to on, off or following its default, and holds the topic's own servers. Screens declare palette entries next to their keys with `useScreenPaletteItems`, the same way they declare hints.
- **Adding an MCP server is a paste (Built):** servers come from READMEs as one thing to copy, so the add sheet is a single box like the palette's, not a form. It reads an address, a command with any `KEY=value` before it, a `claude mcp add`, `claude mcp add-json` or `codex mcp add` line, or a JSON block in the Claude, Cursor, VS Code, Codex or OpenCode shape, with comments and trailing commas (`readPasted` in `apps/dsk/src/mcp.ts`). It shows what it understood, with the name derived from the host or package and editable, and asks only for values docs leave as placeholders like `<token>` or `YOUR_KEY`. An env variable left empty becomes `${KEY}`, so the daemon reads it from its environment. Enter adds every server it found, and opens the browser for each one whose save says it needs a login. The full form is only for editing a server.
- **Images in prompts (Built):** pasting or dropping an image into a message box puts `[Image #3]` at the cursor, and the text is the source of truth: the images that go are the ones it names, so deleting a token drops its image and undo brings it back. Numbers count up through a session, so a number means the same image for the whole conversation. The box stays a plain textarea, so vim, undo and drafts work as before, and `apps/dsk/src/ImageField.tsx` draws the marks on a copy of the text behind it, in the same grid cell so the two wrap alike. Pointing at a token, or putting the cursor beside one, shows the image, `composer.open_image` opens it full size, and Backspace or Delete at a token's edge removes it whole. A tray above the text shows what's attached, and sent messages show their images above them, each behind its own setting: `appearance.images.message_box` and `appearance.images.transcript`. Pasted images bigger than 2000 px on their long side, or over the size model APIs take, are scaled down and re-encoded in the webview before they're sent. The pure token rules are in `apps/dsk/src/imageTokens.ts`.
- **Terminals (Built):** `TerminalPane.tsx` draws with xterm.js and its WebGL renderer, falling back to the DOM when the webview runs out of WebGL contexts. It's only mounted for the shown tab, and its code is a separate chunk loaded two seconds after startup, so it doesn't slow the first paint. The bridge's `terminal_attach` takes a `Channel` and sends the frames that arrive together as one raw message, because each message is a call into the webview. Typing goes through `terminal_input` as the raw request body. The pane acknowledges once per animation frame what xterm.js has written, so a hidden window stops being sent output. Every key goes to the shell except ones starting with Cmd, which stay the app's (`usesCommand` in `keymatch.ts`), so Esc, Ctrl and vim-style sequences reach editors and `global.toggle_sidebar` leaves. Colors come from the theme's `terminal.*` tokens, and the client's theme is what programs asking for the terminal's colors are told. Terminals use the code font. Size, line height, cursor, Option as Meta and history are `appearance.terminal.*` settings.
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
| Terminal output and typing travel as binary frames beside the JSON messages (`protocol::frame`) | Built | What a shell prints isn't valid UTF-8 at every chunk boundary, and escaping it would cost time on every keystroke's echo. Attaching, detaching and resizing stay JSON |
| Diff comments arrive later as new message types | Proposed | Version negotiation already allows this, so they need no work now |
| A response arrives after every state change and followed event its command caused | Built | A client's state is current as soon as a command returns, so UIs can't show a stale status |
| Every enum that can grow has an `Unknown` variant | Built | An older client skips what it doesn't understand instead of failing |
| A client that falls behind gets a fresh snapshot and a refill from the database | Built | A slow client can't hold up the daemon or lose events |
| Image bytes stay with the daemon, as files named by their SHA-256, and events and drafts only refer to them | Built | Transcripts stay small to store and to replay on every subscribe, and the same image has the same ID on every device. A client fetches an image with `read_image` only when it shows it. The daemon refuses an image it can't decode, because an agent would fail the whole turn on it |
| The daemon makes a small copy of each image, at most 640 px on its long side, and clients show that everywhere but full size | Built | A client on a slow link draws a thumbnail without fetching the whole image. It's made when the image is added, or the first time it's asked for. One with transparency stays a PNG, and the rest are JPEG |
| Image files nothing names are removed once they're a day old, at startup and every six hours | Built | Drafts and every recorded prompt count, closed and archived tabs included, because their transcripts can still be read. The day covers an image added a moment before a draft names it, and the new topic form, which keeps its images to itself and sends them again when it starts. Adding an image again starts its day over |
| Prompts are refused while a turn is running, not queued | Built, can change | The simplest honest behavior for now. The Claude adapter reports support for queueing and steering, but other agents may not |

Client behavior that follows from this:

- **Instant open:** the client caches each daemon's last known state, renders it immediately, and catches up in the background.
- **Offline machines:** shown as offline with when they were last seen. Nothing new can be created on them.

## Remote connections

Clients reach daemons on other machines over [iroh](https://iroh.computer) 1.x, without Tailscale or anything else to install or host. The goal is fast and end-to-end encrypted. Hiding how much traffic flows, from the network or from a relay, is not a goal for now. (Settled)

- **Why iroh (Settled):** it dials a device by its Ed25519 public key rather than an address, which is the same thing pairing needs. Connections are QUIC, encrypted end to end, and go direct after hole punching in roughly 9 of 10 networks. The rest go through a relay over HTTPS on port 443, so corporate networks and cellular still work, and QUIC multipath lets a phone move between wifi and cellular without dropping. It's Rust, so it lives in `daemon` and `engine` and reaches iOS through `ffi` like everything else. Its wire protocol is stable across 1.x. WebRTC was ruled out because it's built for browsers and media, needs a signaling server and TURN on top, and tectr has no browser client. A plain relay with our own Noise encryption was ruled out because every byte would go through the relay and the encryption and reconnect glue would be ours to maintain.
- **The default relay (Settled):** n0's public relays and address lookup (`presets::N0`), so pairing works with nothing set up. A relay can't read traffic. It sees IP addresses, when devices connect, and how much is relayed, but nothing once a connection goes direct. A self-hosted relay is a possible later addition.
- **Identity is tectr's (Built):** each daemon and each client has its own Ed25519 key, kept on that machine: the daemon's in its database, a client's in a `0600` file. iroh uses the same key as its address, so over iroh the peer is already authenticated. Nothing else in tectr depends on iroh, which keeps other ways to connect possible.
- **The allowed list (Built):** the daemon keeps the key, a name and when it was paired for every paired client, like `authorized_keys`, in its database, and sends the list to every client as `paired_clients`. A connection from any other key on `tectr/1` is closed with a "not paired" reason before a stream is read. Removing a key (`remove_paired_client`) closes that client's connections straight away, and the client sees "not paired with this device" when it tries again.
- **Pairing (Built):** started on the daemon's own machine: `start_pairing` and `cancel_pairing` are refused from a remote client, so authorizing a new machine needs local access. The code is text, `tectr-<daemon key>-<secret>`, with a 122-bit secret that works once and for 5 minutes, and a new code replaces the last. The new client connects with the ALPN `tectr-pair/1`, sends one JSON `PairRequest` with the secret and its hostname, and gets one `PairResponse`. Without an open code the daemon closes such a connection before reading anything, and 5 wrong secrets use the code up. Each client pairs with each daemon. A QR code for the phone comes with the iOS app. (Proposed)
- **Only when needed (Built):** the daemon starts its iroh endpoint on the first pairing, or at startup once a client is paired, so a machine that's only used locally never reaches the network. It stays up until the daemon stops.
- **Daemon addresses (Built):** a daemon's URL says how to reach it: `ws://` for loopback, `iroh://<key>` for a paired one. `ClientStore` in `engine` keeps the client's key and its paired URLs in `<data dir>/<channel>/client`, and the CLI and the desktop app on one machine share it, so pairing once lets both in. Pairing establishes the key, so a later way to connect is another scheme for the same device, without pairing again.
- **Same protocol on every connection (Built):** over iroh, a connection opens one bidirectional QUIC stream under `tectr/1`, and the same websocket protocol runs over that stream, since `tungstenite` works over any byte stream. The session code in the daemon and `engine` doesn't change. The MCP proxy and login redirects stay loopback only.
- **In `engine` (Built):** `Engine::with_remote` takes the client's key, and binds its iroh endpoint on first use. Opening a connection picks loopback or iroh from the URL, not a plugin system. A later kind, such as Tailscale, is another branch there. One that doesn't authenticate peers by their tectr key adds a handshake proving it, such as Noise IK, before any tectr message. (Proposed)
- **Forgetting a device (Built):** removing a paired device from a client takes the client off the device's allowed list first, if it's online, then drops its URL.
- **Tests (Built):** `Network::Local` binds on loopback with no relay or lookup, and the end-to-end tests fill an iroh `MemoryLookup` with the daemon's address, so they never touch the network.
- **Known gaps:** `transcriptTiming` compares clocks across machines (see Streaming text), so a remote clock running ahead types history. Loopback stays unauthenticated, so the shared-machine gap remains until loopback clients also prove a key. A client waits for every daemon's first attempt before a CLI command runs, which for an unreachable remote device is iroh's connect timeout.

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
| An agent's slash commands are its own, and tectr only lists them | Built | Each agent knows its own commands, including the user's custom ones and skills, so tectr never keeps a list. Running one is sending its text as a prompt |
| Each image goes to the agent after a text block with its token, `[Image #3]` | Built | The agent can tell which image the text means. An agent that doesn't say it takes images fails the turn with a message rather than getting a prompt it can't read |
| Commands that change what tectr sets are left out and refused | Built | `/model`, `/effort`, `/permissions`, `/approvals`, `/config` and `/mcp` would leave tectr showing settings the agent no longer has. The list is by name, not by agent, in `protocol::TECTR_OWNED_COMMANDS` |

**Slash commands (Built, tested against `fake-agent`):** the daemon keeps the list from an agent's `available_commands_update` in `DaemonState.commands`, keyed by session, and in the session's row, so it's there after a restart and a restore, before the agent has started again. It's kept apart from `Session` because a long list would otherwise go out with every status change. A newer list replaces it, closing or archiving the tab drops it, and a list a newer version wrote that doesn't parse is lost until the agent sends it again. Commands that change what tectr sets never make it into the list, and `send_prompt` refuses a prompt that runs one. The app checks the same list, mirrored by hand in `apps/dsk/src/slashCommands.ts` and checked by a test in `protocol`, so the new agent screen doesn't start an agent for a first prompt that would be refused. While the message box holds only `/` and part of a name, it lists the commands whose names start with that, then those containing it. `composer.command_up` and `composer.command_down` move, `composer.complete_command` completes, and the send key sends a command that takes no input straight away and completes one that does. One extra stop with nothing highlighted sends what's typed as it is. In vim's normal mode the list waits for insert mode. Only a tab's own agent's list is ever shown, because that's the only one certain to be right: another tab's agent can be working in a project with different commands. So the new agent screen suggests nothing, since there's no agent yet. The Claude adapter runs `/usage` itself and answers with Markdown in ordinary message chunks, so it needs nothing from tectr. A brand-new tab has no list of its own until its agent has started.

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
- **Claude's own history:** Claude Code stores each conversation under `~/.claude/projects/`, keyed by working directory, and not in tectr's data directory. So tectr sessions appear in the user's Claude history, and that history stays when a topic's worktree is removed. Restoring an archived topic makes its folder again at the same path, so its tabs resume there.
- **Resuming in another folder (tested 2026-11-09, with the Claude Code CLI and Haiku):** `--resume` from a different folder finds the conversation by its ID, keeps its memory, and works in the new folder, while new turns go on being stored under the old folder's history. That's what moving a topic relies on. The system prompt names the folder and its git status, so the prompt cache misses once after a move. Claude Code caches for an hour, so a move within that costs one full write of the conversation.
- **Async runtime:** the SDK runs fine under Tokio. Its callbacks run in the connection's dispatch loop, so the daemon stores a permission responder and returns instead of waiting. A connection doesn't report a clean agent exit as an error, so the daemon has to watch `incoming_closed()` itself.
- **Cancel:** cancelling answers open permission requests, as ACP requires, and stopped a running `sleep` within about a second. A tool the model starts in the background can outlive the turn, because the adapter leaves background commands running.
- **Authentication:** `authMethods` is empty and the adapter silently uses the machine's Claude Code login. On a machine logged in with a subscription, tectr therefore drives Claude Code with that subscription, which is exactly the terms question listed under open questions. Answer it before anyone else uses this.
- **Not forwarded yet:** plans, usage, and config changes are dropped. Agent questions other than permission requests aren't handled, so answering them is still to build.
- **Pinning is partial:** the daemon pins the adapter's own version, but npm can still pick newer versions of the adapter's dependencies. A checked-in lockfile would close this.

## MCP servers

tectr owns every MCP server agents get. Each agent's own MCP configuration is switched off, and the daemon passes the servers of the session's topic over ACP instead, so Claude Code, Codex and OpenCode all get the same ones. The exception is an agent's connectors: the claude.ai connectors of Claude and the ChatGPT apps of Codex, whose logins Anthropic and OpenAI hold. tectr lists them and turns each one on or off like its own servers, and the agent connects to the ones that are on by itself. (Proposed)

| Decision | Status | Why |
|---|---|---|
| Servers are device settings in the daemon, either device-wide or a topic's own. A topic turns each device-wide server on or off against that server's own default. An archived topic keeps its own servers, so restoring it brings them back | Proposed | Every client sees the same servers, and a topic only needs to say how it differs |
| Agents' own servers are switched off. Their connectors are servers in tectr's list that only their own agent gets, and new ones start off | Proposed | One list for every agent instead of a merge nobody can see. Connectors can't go through the proxy, because tectr never has their logins, so the agent is let through to the ones that are on instead |
| HTTP servers go through a proxy in the daemon, which adds the login | Proposed | Tokens never reach an agent process, its logs or its transcript, and a refresh needs no restart |
| The daemon logs in with OAuth itself, and keeps logins in its database | Proposed | One login serves every agent. See Logins below for why not the Keychain |
| An agent whose servers changed is stopped when it's idle, or after its turn, and its next prompt resumes it with the new ones | Proposed | ACP can't change a running session's servers |

- **Switching agents' servers off (Proposed):** with none of its connectors on, Claude gets `strictMcpConfig` through the adapter's `_meta.claudeCode.options`, which leaves out config files, plugins and agent definitions, and `ENABLE_CLAUDEAI_MCP_SERVERS=false`, which stops Claude Code fetching connectors. Strict mode leaves out connectors too, so with one on, Claude instead gets an `allowedMcpServers` list in the adapter's `settings` option, and `ENABLE_CLAUDEAI_MCP_SERVERS=true` in case the daemon's own environment says false. The list names tectr's stdio servers, the proxy address of each of its HTTP servers, and the address of each connector that's on. Claude Code matches a stdio server by name, and once the list has any address, every remote server and connector by address only. A connector's address is its service's own, like `https://mcp.slack.com/mcp`, not Anthropic's proxy. Everything else Claude Code would load is blocked. Codex gets `-c features.plugins=false` and `-c mcp_servers.<name>.enabled=false` for each server in `~/.codex/config.toml` and the project's `.codex/config.toml`. With none of its apps on it gets `-c features.apps=false`, and with some, `-c features.apps=true -c apps._default.enabled=false` and `-c apps.<id>.enabled=true` for each, so an app connected since the last refresh stays off. `-c mcp_servers={}` would do nothing, because overrides merge key by key. OpenCode gets `OPENCODE_CONFIG_CONTENT` with `enabled: false` for each server in its global config and in every folder from the project up to the git root, merged into what that variable already held. Codex and OpenCode can only be switched off by name, so their files are read each time one starts, and a server from a place the daemon doesn't read would slip through. A server of theirs with the same name as one of tectr's is left alone, because tectr's replaces it. Probes for a provider's models get the same treatment. The Codex and OpenCode switches were checked with `codex mcp list` and `opencode mcp list`. Claude's strict mode is read from the adapter and SDK code and hasn't been checked against a real session yet. The allowlist was checked by running the Claude Code the adapter ships (2.1.316) with the same settings as flags, and the app switches by asking Codex CLI 0.166 for its tools, not through the adapters.
- **Values (Proposed):** a value can name the daemon's environment as `${NAME}`, filled in when an agent starts or a request is proxied. That's the login shell's environment with `--shell-env`. Every client of the daemon sees the values themselves, so secrets belong in variables. Today that's only loopback clients of the same user, but it matters once pairing exists.
- **The proxy (Proposed):** it shares the daemon's loopback port. A connection whose first request line asks for `/mcp/` or `/oauth/` is served as plain HTTP, and anything else goes on to the websocket handshake, so there's no second listener. An agent gets `http://127.0.0.1:<port>/mcp/<secret>/<server>`, with a secret made for that agent process that dies with it. Requests with an `Origin` header are refused, as on the websocket. Only the headers MCP needs pass in either direction. `www-authenticate` is held back, so an agent never tries to log in by itself, and a server that turns the daemon away marks itself as needing a login and gets a JSON-RPC error saying so. A token the server refuses is refreshed once and the request sent again.
- **Logins (Proposed):** the daemon follows the MCP authorization spec: the server's protected resource metadata, then its authorization server's metadata, then dynamic client registration, falling back to the 2025-03-26 defaults when there's no metadata. It uses PKCE with S256 and sends `resource` with every request. A server that doesn't register clients takes a client ID entered by hand, and client secrets aren't supported. Saving an HTTP server waits up to 5 seconds to ask it without a login, so the saved server already says whether it needs one and a client can open the browser straight away. A slower server is asked in the background instead. A login starts with `login_mcp_server`, which returns the address for a client to open, and finishes when the browser comes back to `http://localhost:<port>/oauth/callback` on the daemon. `complete_mcp_login` finishes one with a code a client caught itself, which a remote daemon will need, because there the browser and the daemon aren't on the same machine. Logins are kept in the database, in a table separate from the servers, and never leave the daemon. The macOS Keychain would ask the person each time the daemon's binary changes, which is every build and every upgrade while it isn't signed, and a daemon nobody is looking at can't answer that.
- **Connectors (Proposed):** `refresh_mcp_connectors` lists the connectors of each added Claude and Codex agent and keeps their servers in step: a new one is added off, a gone one is removed, and one that's still there keeps its own on or off and every topic's. An agent that can't be listed keeps the connectors it had. Claude's come from `claude mcp list`, run with the Claude Code the adapter ships, in an empty folder. Codex has no command for its apps, so they come from the tools it caches in `~/.codex/cache/codex_apps_tools/`, which only Codex itself fills in. OpenAI's built-in apps there, like Pets, are left out. A connector keeps its agent's name for it, `claude.ai Slack` or `ChatGPT Slack`, which no other server can have, and can only be turned on or off. Refreshing is never automatic, because `claude mcp list` starts every server Claude Code has to check it.
- **Import (Proposed):** `import_mcp_servers` copies the servers in `~/.claude.json`, `~/.codex/config.toml` and OpenCode's global config, as device-wide servers. SSE servers are skipped, because the proxy only speaks streamable HTTP, and so is a name that's already taken.
- **Agents without HTTP (Proposed):** an agent whose ACP capabilities don't include HTTP servers starts without them, and its session shows an error naming what it left out.

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
├── design/                 # shared tokens, icons, logo, specs, fixtures
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
| Terminal engine | libghostty-vt, built by Zig 0.17.1 from mise, through the `libghostty-vt` bindings pinned to the revision for its `vt-1.0.0` release. `.cargo/config.toml` always builds it with `ReleaseFast` | Built | Its formatter recreates a screen for a client attaching, which would otherwise be code of our own. A debug build of it is many times slower, which made terminals in dev builds fall behind. Zig 0.15 can't link on macOS 27. The build script clones Ghostty at its pinned commit, so a clean build needs the network |
| Monorepo orchestrator | None (no Turborepo, Nx, Moon, or Bazel) | Proposed | One JS app, Cargo handles incremental Rust builds, and only two steps cross languages. Revisit if JS packages multiply or CI time becomes a problem |
| iOS project | XcodeGen | Proposed | Avoids `.pbxproj` merge conflicts |
| iOS bindings | Local `uniffi-bindgen` binary and a mise task that builds the xcframeworks; generated files stay out of git | Proposed | Keeps the bindgen version in step with the library |
| CI | GitHub Actions with path filters, `rust-cache`, `cargo-deny`, `mise-action` | Proposed | The expensive macOS iOS job runs only when `apps/ios`, FFI, or their dependencies change |
| Releases | One version for the app, CLI and daemon, tagged like `v0.1.0`. `mise run release <version>` builds on this Mac, which needs Xcode 26 or later for the Liquid Glass app icon, uploads to GitHub Releases on the public repo, and updates the cask in `johannes-krabbe/homebrew-tap`. Apple Silicon only, ad-hoc signed and not notarized, so the cask removes the quarantine flag | Built | They ship together in one cask, and local builds cost no CI minutes. Without a Developer ID certificate there's no notarization, and macOS may ask again for permissions after an update. Revisit per-component tags when iOS ships |

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
- A terminal sends a client only what it can draw, and the screen as it is once it catches up, never a backlog.
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

- Pairing, remote connections, and the push relay. (Settled)
- Tabs, terminals, diff comments, and adding workspaces from a git URL or a new repo. (Proposed)
- The twig decision, because plain `git worktree add` is enough. (Proposed)

### After that

(Proposed, except where noted)

1. `ffi`, and the empty iOS app showing the session list, which only tests the iOS build and bindings. (Settled that this comes soon after the milestone)
2. Pairing and remote connections over iroh. (Built)
3. `push`, `push-ffi`, and `push-relay`.

## Open questions

From `idea.md`. The terms question now matters in practice, because the adapter uses the machine's Claude login by default:

- **Auth and terms:** what do Anthropic's terms allow when a third-party client drives Claude Code with a claude.ai subscription rather than an API key?
- **Relationship to twig:** should the daemon call twig, which is written in Go, as a separate process, or reimplement its worktree logic? Worktree logic can start as a daemon module either way.
- **Desktop and phone layout:** the desktop UX isn't settled, and the phone app hasn't been redesigned around workspaces and topics.

Raised during design:

- **Permission modes:** does auto mode still send some requests to the user, with only yolo never asking? `idea.md` line 16 no longer mentions approving tool calls. The working assumption is yes, and the daemon relays whatever the agent asks in any mode. How often Claude's auto mode asks hasn't been measured. It's described as "Claude handles permission decisions".
- **Draft diff comments:** do they live on the daemon or the client? This decides whether a review started on desktop can be continued on the phone.

## Possible later additions

- **Self-hosted relay:** `iroh-relay` on your own server, chosen as a setting instead of the public default, so no third party relays your traffic or sees how much of it there is.
- **Other ways to connect:** Tailscale or similar, as another kind of connection next to iroh (see Remote connections).
- **Hub for team views or a web client:** an optional aggregator that connects to daemons as a client and offers the same protocol to apps, which treat it as just another daemon.
