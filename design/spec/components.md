# Components

Each component has the same name, parts, and states on both platforms, and is built separately on each. These are the pieces that hold up whatever layout wins. Add to this list as the layouts settle.

Token names are shortened: `text.primary` means `color.text.primary`.

## StatusIcon

One icon per state, from `design/icons/`. Each is a single color, and inner shapes are cut-outs, so they work on any surface and as iOS template images.

| State | Color | Meaning |
|---|---|---|
| `working` | `status.working` | The agent is running |
| `waiting` | `status.waiting` | It needs you |
| `failed` | `status.failed` | It stopped with an error |
| `done` | `status.done` | It finished |
| `idle` | `status.idle` | Nothing is running |
| `stale` | `status.stale` | Its device is offline, so this is the last known state |

A topic shows its most urgent session, in this order: waiting, failed, working, done, idle, stale.

Size: `size.icon`. Desktop uses the SVGs directly, and iOS imports them as template images in an asset catalog.

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
- **Unfolded:** the steps below it, on a `line.subtle` rule. A key or the palette does this for every activity, and pressing or clicking a line does it for one.
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

## Sheet

Creates something with just a name, with defaults for the rest.

- **Parts:** where it will be created, name input in `text.title`, option chips, footer.
- **Surface:** `surface.overlay`, `line.strong` border, `shadow.sheet`, over `scrim`.
- **iOS:** a system `.sheet`, using our tokens only for content.

## Button

- **Primary:** background `accent.solid`, text `text.on-accent`. At most one per view.
- **Secondary:** background `surface.control`, hover `surface.control-hover`.
- **Height:** `size.control`.
- **iOS:** native button styles, such as `.glass` and `.glassProminent`, tinted with `accent.solid` only on the primary action.

## Kbd

Desktop only. A key hint in `text.meta`, on `surface.control`. iOS has no equivalent.
