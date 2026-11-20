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
- **Reduced motion:** the arc stands still, which still reads differently from idle, and states change without moving. `appearance.motion.status_icons` turns it off too.

Size: `size.icon`. Desktop composes the SVGs into layers in `StatusIcon`, and iOS imports them as template images in an asset catalog.

## Icon

Every other icon comes from Lucide through `apps/dsk/src/ui/Icon.tsx`, by a name of our own like `settings` or `add`. Nothing else imports the library, which a test checks, so swapping an icon or drawing one by hand is one line in that file.

- **Size:** `size.icon`, with a 1.5px stroke at any size.
- **Hand-drawn icons:** only where no library icon fits or where it has to move. Draw them on Lucide's 24px grid with round caps and joins, so they sit next to it.
- **iOS:** SF Symbols, mapped from the same names.

## DeviceHeader

The fixed section header that groups workspaces. It never collapses and isn't indented.

- **Parts:** online dot, name, last seen when offline.
- **States:** online (`status.online` dot), not responding (the same dot, "Not responding" in `status.waiting`), offline (hollow dot, name in `text.tertiary`).
- **Desktop:** behaves like a WorkspaceRow: reaching it with the keyboard shows the device's page, offline or not, and Enter or a click goes into it. Its gear opens its settings and its + adds a workspace, neither opening the page.
- **iOS:** a `List` section header, with the device's settings and add buttons at its end.

## WorkspaceRow

- **Parts:** name, count of waiting sessions when folded.
- **States:** open, folded, hover (desktop), selected.
- **Desktop:** behaves like a TopicRow: reaching it with the keyboard shows the workspace's page, and Enter or a click goes into it. Its + adds a topic without opening the page. Right click opens its menu: a new topic, and closing it, which is also `d d`.
- **iOS:** a row in the tree, with its topics indented below and a button for a new topic.

## DevicePage

A device's own page in the main pane, built like a WorkspacePage: its workspaces, and the closed ones.

- **Parts:** header (name in `text.title`, then how it is in `text.tertiary`, or `status.waiting` when it's not responding, then "Keep awake" with its dropdown, and a menu button that adds a workspace and opens its settings), then a Workspaces section, and "Closed workspaces" as a disclosure under it.
- **Workspace rows:** one Tab stop. StatusIcon, name, the path with `~` in `text.code-small` and `text.tertiary`, the most urgent state of its topics in words when it's waiting, failed or working, with how many topics share it, and the number of topics. A menu button at the end shows on the row in use and is a Tab after it. Right click opens the same menu, and `d d` closes the workspace while its row has the keyboard.
- **Closed rows:** form rows with the name in `text.tertiary`, the path, how many topics and when it closed, Open as the row's button, and a menu with Remove that shows on the row in use.

## WorkspacePage

A workspace's own page in the main pane: its topics, and the MCP servers its agents get.

- **Parts:** header (name in `text.title`, then the device and the path with `~` for the home folder, both in `text.tertiary`, and a menu button with every page action), then a Topics section and an MCP servers section. A section heading is its name in `text.secondary`, a count in `text.tertiary`, a line under it, and one quiet add button at the end. There's no bar at the bottom.
- **Keys stay out of sight:** rows and buttons never show key chips. Tooltips and menus name the keys of the few actions that have one.
- **Rows:** the topics are one Tab stop. The row keys of settings move through every row of the page, topics and servers, Enter opens a topic or presses a server's first control, and left and right turn a server off and on. A server row is a form row, with its switch, profile and menu used where they are. Pointing at a row or focusing into it highlights it, and nothing is highlighted otherwise, so landing on a workspace shows a calm page.
- **Topic rows:** StatusIcon, name, and the number of tabs. Only waiting and failed get words, because idle is what the icon already says.
- **Server rows:** name, what it runs in `text.code-small` and `text.tertiary`, a Log in button in `status.waiting` when it needs one, the profile as a quiet dropdown, a switch, and a menu button. The menu button, and the profile dropdown of a server without profiles, only show on the row in use. Right click opens the same menu. A server turned off has its name in `text.tertiary`.
- **Empty sections:** a row in `text.tertiary` with an icon that does the obvious thing ("New topic", "Add an MCP server"), reachable with the arrows like any row. "Copy from another workspace" is a second such row, and only shows when another workspace has servers to copy, which is the one time it matters. With servers in the list, copying is in the page menu.
- **Adding:** each section's heading has its add button, in the Tab order, and an empty section's row does the same. Nothing on the page has a letter key of its own.
- **Archived topics:** a quiet disclosure under the topics, in `text.tertiary` with a chevron, expands them in place as dim rows, each with Restore. The page menu opens it too.
- **Add from the library:** a disclosure under the servers, with how many can be added. Open, it lists them as rows with Add, then "New server..." and a Copy from dropdown when another workspace has servers to copy. It starts open while the workspace has none.
- **Only its own servers:** the list shows what the workspace added. Everything else in the library waits in the disclosure below it.

## TopicRow

- **Parts:** StatusIcon, name, meta (age or short status).
- **States:** default, waiting, failed, done, stale, hover (desktop), selected.
- **Waiting:** background `status.waiting-tint`, or `status.waiting-tint-strong` when selected or hovered. Meta in `status.waiting`. Never use `text.tertiary` on the strong tint, because it fails contrast in dark mode.
- **Selected:** background `surface.control`. On desktop, when the sidebar owns the keyboard, `accent.tint` with a 1px `accent.ring` border.
- **Height:** `size.row`.
- **Right click (desktop):** a Menu of what can be done to the topic, the same one the tab bar has.

## Sidebar

Desktop only. The tree of devices, workspaces and topics, which is how topics are found, with one row below it.

- **Tree:** one Tab stop, the open topic's row, with the arrows or `j` and `k` moving through it. On a row, `a` adds what goes under it, a workspace on a device and a topic on a workspace or topic, the same as its + button. A topic row also takes `r`, `d d` and `u`, which its right click menu names.
- **Empty rows:** "Add a workspace" and "Create a topic" in `text.tertiary` with the add icon, where a device or workspace has nothing yet.
- **Foot:** Settings, with its key in the tooltip, and, while there's a request waiting or a topic unread, an icon button with the Menu of what's about every topic at once. Both are in the Tab order.

## TabBar

Desktop only. A topic's tabs, then the button for a new one, with the topic's Menu fixed at the end while the tabs scroll.

- **Tab:** StatusIcon or the terminal icon, then the label, which fades out when it doesn't fit. The shown one is `text.primary` with an `accent.text` line under it.
- **Close:** a small button after the label, shown while the pointer is on the tab. Its room is always there, so the tabs after it don't move. A tab that can't be closed has none.

## Menu

Desktop only. A short list of actions, opened by an icon button or a right click. It's where what's done now and then lives, so those actions need no key and no chrome of their own. Its button is in the Tab order.

- **Surface:** the same as a dropdown: `surface.overlay`, `line.strong` border, `shadow.sheet`.
- **Rows:** the action in `text.label`, and its key on the right for the few that have one, `size.row` tall. The highlighted row is `surface.control`.
- **Groups:** a `line.subtle` rule between them. What can't be undone easily, like archiving, is last and on its own.
- **Submenus:** an item with a few choices behind it, like "Move to another folder", ends in a chevron and opens them beside the menu, by pointer or with right or `l`.
- **Wording:** an item names only the action ("Rename"), because what it acts on is where the menu was opened. Items that would do nothing are left out.
- **iOS:** a system context menu.

## RequestCard

Something an agent needs from you. In auto mode this should be rare, and in yolo mode it's only ever a question. Whether auto mode still asks is an open question in `architecture.md`.

- **Parts:** StatusIcon and kind, position ("1 of 2 waiting"), the command or question in `text.code-body`, the reason in `text.body` and `text.secondary`, actions.
- **Surface:** `status.waiting-tint` with a `status.waiting-border` border, on desktop. On iOS the glass the message box sits on.
- **Actions:** one primary Button, the rest secondary. On desktop each starts with its number in `text.tertiary`, which is also the key that answers with it, so the key is learned from the card rather than a chip. On iOS they're bordered rather than glass, because the card is already on glass, and sit side by side when their names fit.
- **iOS:** in place of the message box until it's answered, then the next request for that agent, then the box again.

## ActivityLine

What an agent does between its messages, as one line in `text.meta`. Messages are never folded, so you can read along between steps.

- **While running:** at the end of the transcript, it names the newest step still going in `text.tertiary`, with a `text.primary` highlight sweeping across it every `motion.sweep`. Each step fades out before the next fades in, and stays up long enough to read.
- **Waiting:** "Waiting for you" in `status.waiting`, without the sweep.
- **Finished:** how long it took and how many steps, such as "Worked for 12s, 6 steps", with failures in `status.failed`. Time spent on a permission request doesn't count.
- **Unfolded:** the steps below it, on a `line.subtle` rule. `conversation.steps`, also in the tab bar's menu, does this for every activity, and pressing or clicking a line does it for one. Clicking the live line does it for the whole turn so far and for activities it starts later, so you can watch an agent work across its messages.
- **Reduced motion:** no sweep, and messages and scrolling don't animate either.
- **Settings:** `conversation.steps` and `appearance.motion.transcript`.

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

What a topic's new tab shows: what to open next, as a short list in the middle of the pane, moved through with the arrows.

- **Rows:** icon in `text.tertiary`, label, and the kind's letter on the right where it has one, `size.row` tall. The focused row is `surface.control`, so the choice shows however the tab was opened.
- **Not built yet:** shown, disabled, with a meta note, so the list stays put as kinds arrive.
- **After choosing:** kinds that need setting up, like an agent, replace the list with their form. Kinds that don't open straight away.

## Sheet

Creates something with just a name, with defaults for the rest.

- **Parts:** where it will be created, name input in `text.title`, footer.
- **Surface:** `surface.overlay`, `line.strong` border, `shadow.sheet`, over `scrim`.
- **With fields:** when there's more to fill in, like an MCP server, labelled fields follow the name in two columns: the label in `text.meta` and `text.tertiary`, the input in `text.code-body` on `surface.raised` with a `line.strong` border that turns `accent.text` while focused. A box of lines (`KEY=VALUE`) takes Enter as a new line, so a save key with a modifier works from every field. A choice between a few values is a row of small buttons, the chosen one on `surface.control`, each in the Tab order.
- **As a list:** a sheet can hold a list instead, like the MCP servers to give a new workspace, filtered by what's typed. A list where several can be chosen marks each with a check box, `accent.solid` when checked, and keeps the footer's buttons.
- **iOS:** a system `.sheet`, using our tokens only for content.

## Button

- **Primary:** background `accent.solid`, text `text.on-accent`. At most one per view.
- **Secondary:** background `surface.control`, hover `surface.control-hover`.
- **Height:** `size.control`.
- **iOS:** native button styles, such as `.glass` and `.glassProminent`, tinted with `accent.solid` only on the primary action.

## Toast

A result that needs nothing from you, for a few seconds.

- **Parts:** one line in `text.label`, cut short with an ellipsis if it doesn't fit.
- **Surface:** `surface.overlay`, `line.strong` border, `shadow.popover`, centered at the bottom of the window.
- **Behavior:** it fades in over `motion.normal` and goes after `general.toast_duration`. It never takes a click or the keyboard. The same text again keeps one toast, and at most three are stacked.
- **Not for failures:** those stay on screen, where the thing that failed is.
- **iOS:** a glass capsule at the top, over sheets too.

## Kbd

Desktop only. A key in `text.meta`, on `surface.control`, in menus and the new tab's rows, the only places a key shows outside a tooltip. iOS has no equivalent.
