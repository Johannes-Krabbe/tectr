# Components

Each component has the same name, parts, and states on both platforms, and is built separately on each. These are the pieces that hold up whatever layout wins. Add to this list as the layouts settle.

Token names are shortened: `text.primary` means `color.text.primary`.

## StatusIcon

One icon per state, from `design/icons/`. Each is a single color, and inner shapes are cut-outs, so they work on any surface and as iOS template images. They're drawn by hand, not taken from the icon library.

| State | Color | Meaning |
|---|---|---|
| `working` | `status.working` | The agent is running. An arc turns around a faint ring |
| `waiting` | `status.waiting` | It needs you |
| `failed` | `status.failed` | It stopped with an error |
| `done` | `status.done` | It finished and you haven't looked yet. Green, the theme's `status.online` |
| `read` | `status.read` | It finished and you've seen it. A gray outline check |
| `idle` | `status.idle` | Nothing is running |
| `stale` | `status.stale` | Its device is offline, so this is the last known state |

A topic shows its most urgent session, in this order: waiting, failed, done, working, read, idle, stale. An unread result is something for you to do and a working agent isn't, so it comes first.

- **Read:** a session is unread from the moment a turn finishes until it has been shown for a second in a focused window with the keyboard in the main pane, or someone marks it read. Moving over its topic in the sidebar shows it without reading it. The flag lives on the session in the daemon, so reading on one client reads everywhere. Starting new work clears it. Failed has no read state.
- **Motion:** desktop puts every state in one SVG as layers and shows the current one, so a row's icon moves between states. Finishing closes the arc into a full ring that then fills with the check. Reading drains the fill over `motion.fade`. Every turning arc is pinned to the document timeline, so they turn in step.
- **Reduced motion:** the arc stands still, which still reads differently from idle, and states change without moving. `appearance.layout.animate_status` turns it off too.

Size: `size.icon`. Desktop composes the SVGs into layers in `StatusIcon`, and iOS imports them as template images in an asset catalog.

## Icon

Every other icon comes from Lucide through `apps/dsk/src/ui/Icon.tsx`, by a name of our own like `settings` or `add`. Nothing else imports the library, which a test checks, so swapping an icon or drawing one by hand is one line in that file.

- **Size:** `size.icon`, with a 1.5px stroke at any size.
- **Hand-drawn icons:** only where no library icon fits or where it has to move. Draw them on Lucide's 24px grid with round caps and joins, so they sit next to it.
- **iOS:** SF Symbols, mapped from the same names.

## DeviceHeader

The fixed section header that groups workspaces. It never collapses and isn't indented.

- **Parts:** online dot, name, last seen when offline.
- **States:** online (`status.online` dot), offline (hollow dot, name in `text.tertiary`).
- **iOS:** a `List` section header.

## WorkspaceRow

- **Parts:** name, count of waiting sessions when folded.
- **States:** open, folded, hover (desktop), selected.
- **iOS:** a row that pushes its topics.

## TopicRow

- **Parts:** StatusIcon, name, meta (age or short status).
- **States:** default, waiting, failed, done, stale, hover (desktop), selected.
- **Waiting:** background `status.waiting-tint`, or `status.waiting-tint-strong` when selected or hovered. Meta in `status.waiting`. Never use `text.tertiary` on the strong tint, because it fails contrast in dark mode.
- **Selected:** background `surface.control`. On desktop, when the sidebar owns the keyboard, `accent.tint` with a 1px `accent.ring` border.
- **Height:** `size.row`.

## RequestCard

Something an agent needs from you. In auto mode this should be rare, and in yolo mode it's only ever a question. Whether auto mode still asks is an open question in `architecture.md`.

- **Parts:** StatusIcon and kind, position ("1 of 2 waiting"), the command or question in `text.code-body`, the reason in `text.body` and `text.secondary`, actions.
- **Surface:** `status.waiting-tint` with a `status.waiting-border` border, on desktop. The surrounding sheet on iOS.
- **Actions:** one primary Button, the rest secondary.
- **iOS:** a `.sheet` with detents, with glass from the system.

## ActivityLine

What an agent does between its messages, as one line in `text.meta`. Messages are never folded, so you can read along between steps.

- **While running:** at the end of the transcript, it names the newest step still going in `text.tertiary`, with a `text.primary` highlight sweeping across it every `motion.sweep`. Each step fades out before the next fades in, and stays up long enough to read.
- **Waiting:** "Waiting for you" in `status.waiting`, without the sweep.
- **Finished:** how long it took and how many steps, such as "Worked for 12s, 6 steps", with failures in `status.failed`. Time spent on a permission request doesn't count.
- **Unfolded:** the steps below it, on a `line.subtle` rule. A key or the palette does this for every activity, and pressing or clicking a line does it for one. Clicking the live line, or `pane.toggle_turn`, does it for the whole turn so far and for activities it starts later, so you can watch an agent work across its messages.
- **Reduced motion:** no sweep, and messages and scrolling don't animate either.
- **Settings:** `appearance.transcript.steps` and `appearance.transcript.animate`.

## DiffHunk

- **Parts:** header in `text.tertiary`, lines in `text.code-small`, comments between lines.
- **Line colors:** `diff.add-bg` and `diff.del-bg` for backgrounds. Text comes from `syntax.*` roles when spans are available, and from `diff.add-text` and `diff.del-text` when they aren't.
- **States:** default, focused (`accent.text` outline), commenting.

## ReviewBar

Diff review is a major feature: you leave comments on many things, then send them all at once.

- **Parts:** count of draft comments, a primary "Send N comments" Button, discard.
- **Rule:** comments are drafts until sent, and nothing reaches the agent before that.
- **iOS:** a bottom toolbar, in glass.

## NewTab

What a topic's new tab shows: what to open next, as a short numbered list in the middle of the pane.

- **Rows:** icon in `text.tertiary`, label, and the key on the right, `size.row` tall. The focused row is `surface.control`, so the choice shows however the tab was opened.
- **Not built yet:** shown, disabled, with a meta note, so the numbers stay put as kinds arrive.
- **After choosing:** kinds that need setting up, like an agent, replace the list with their form. Kinds that don't open straight away.

## Sheet

Creates something with just a name, with defaults for the rest.

- **Parts:** where it will be created, name input in `text.title`, footer.
- **Surface:** `surface.overlay`, `line.strong` border, `shadow.sheet`, over `scrim`.
- **With fields:** when there's more to fill in, like an MCP server, labelled fields follow the name in two columns: the label in `text.meta` and `text.tertiary`, the input in `text.code-body` on `surface.raised` with a `line.strong` border that turns `accent.text` while focused. A box of lines (`KEY=VALUE`) takes Enter as a new line, so a save key with a modifier works from every field. A choice between a few values is a row of small buttons, the chosen one on `surface.control`, with its key after them.
- **As a list:** a sheet can hold a list instead, like a topic's MCP servers, with group headings in `text.meta`. What the highlighted row can do sits in a bar above the footer, the same as the settings actions bar, so the list itself doesn't move.
- **iOS:** a system `.sheet`, using our tokens only for content.

## Button

- **Primary:** background `accent.solid`, text `text.on-accent`. At most one per view.
- **Secondary:** background `surface.control`, hover `surface.control-hover`.
- **Height:** `size.control`.
- **iOS:** native button styles, such as `.glass` and `.glassProminent`, tinted with `accent.solid` only on the primary action.

## Kbd

Desktop only. A key hint in `text.meta`, on `surface.control`. iOS has no equivalent.
