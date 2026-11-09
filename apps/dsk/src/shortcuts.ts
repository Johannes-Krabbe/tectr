// What the shortcut line shows. Each screen says which of its keys work right now, next to
// the handlers that run them, and the line picks by what has the keyboard.

import { useLayoutEffect, useSyncExternalStore } from "react";
import type { ActionId } from "./keybinds";

/** Actions shown together as one key label, such as up and down as K/J. Without actions it's
 * a note about what to do. */
export interface Hint {
  label: string;
  actions: ActionId[];
}

export const hint = (label: string, ...actions: ActionId[]): Hint => ({ label, actions });

/** `overlay` takes the keyboard from everything behind it, and the latest one is on top.
 * `pane` and `typing` are the main pane without and with a text field focused. `global` is
 * added to whichever of those is showing, and `pinned` stays visible however narrow it gets. */
export type HintPlace = "overlay" | "pane" | "typing" | "global" | "pinned";

export interface HintEntry {
  place: HintPlace;
  hints: Hint[];
}

/** Who has the keyboard, as `keyboardPlace` in keys.ts reports it. */
export type KeyboardPlace =
  | "pane"
  | "typing"
  | "sidebar"
  | "sidebar-device"
  | "sidebar-workspace"
  | "sidebar-topic"
  | "sidebar-empty";

const SIDEBAR_MOVES = [
  hint("Move", "sidebar.up", "sidebar.down"),
  hint("Half screen", "sidebar.half_up", "sidebar.half_down"),
  hint("First, last", "sidebar.first", "sidebar.last"),
  hint("Center", "sidebar.center"),
];

// What confirming and adding do depends on the row, as `onSidebarKey` and `pressAdd` decide.
function sidebarHints(place: KeyboardPlace): Hint[] {
  switch (place) {
    case "sidebar-device":
      return [
        hint("Device settings", "sidebar.confirm"),
        hint("Add workspace", "sidebar.add"),
        ...SIDEBAR_MOVES,
      ];
    case "sidebar-workspace":
      return [
        hint("Add topic", "sidebar.add"),
        hint("Device", "sidebar.parent"),
        ...SIDEBAR_MOVES,
      ];
    case "sidebar-topic":
      return [
        hint("Open", "sidebar.confirm", "sidebar.open"),
        hint("Add topic", "sidebar.add"),
        hint("Rename", "sidebar.rename"),
        hint("Delete", "sidebar.delete"),
        hint("Workspace", "sidebar.parent"),
        ...SIDEBAR_MOVES,
      ];
    case "sidebar-empty":
      return [hint("Add", "sidebar.add"), ...SIDEBAR_MOVES];
    default:
      return SIDEBAR_MOVES;
  }
}

export function chooseHints(
  entries: HintEntry[],
  place: KeyboardPlace,
): { hints: Hint[]; pinned: Hint[] } {
  const inPlace = (wanted: HintPlace) =>
    entries.filter((entry) => entry.place === wanted).flatMap((entry) => entry.hints);
  const overlay = entries.filter((entry) => entry.place === "overlay").at(-1);
  if (overlay) return { hints: overlay.hints, pinned: [] };
  const local = place === "pane" || place === "typing" ? inPlace(place) : sidebarHints(place);
  return { hints: [...local, ...inPlace("global")], pinned: inPlace("pinned") };
}

/** Moving through the rows of a settings section. */
export const SETTINGS_MOVES: Hint[] = [
  hint("Move", "settings.up", "settings.down"),
  hint("Half screen", "settings.half_up", "settings.half_down"),
  hint("First, last", "settings.first", "settings.last"),
  hint("Center", "settings.center"),
  hint("Back", "settings.back", "settings.parent"),
];

const TAB_ACTIONS: ActionId[] = [
  "global.tab_1",
  "global.tab_2",
  "global.tab_3",
  "global.tab_4",
  "global.tab_5",
  "global.tab_6",
  "global.tab_7",
  "global.tab_8",
  "global.tab_9",
];

const OPTION_ACTIONS: ActionId[] = [
  "permission.option_1",
  "permission.option_2",
  "permission.option_3",
  "permission.option_4",
  "permission.option_5",
  "permission.option_6",
  "permission.option_7",
  "permission.option_8",
  "permission.option_9",
];

/** The number keys that answer a permission request with this many options. */
export const optionNumbersHint = (count: number): Hint =>
  hint("Option", ...OPTION_ACTIONS.slice(0, count));

/** What the keys that work everywhere can do at the moment. A key that would do nothing
 * isn't shown, which is also why tabs only go as far as there are tabs. */
export function globalHints(now: {
  interrupt: boolean;
  tabs: number;
  closeTab: boolean;
  newAgent: boolean;
  newTopic: boolean;
  newWorkspace: boolean;
  topics: number;
  workspaces: number;
  sidebar: boolean;
  settings: boolean;
}): Hint[] {
  const hints: Hint[] = [];
  if (now.interrupt) hints.push(hint("Interrupt", "global.interrupt"));
  if (now.tabs > 1) {
    hints.push(hint("Tab", ...TAB_ACTIONS.slice(0, now.tabs)));
    hints.push(hint("Previous, next tab", "global.prev_tab", "global.next_tab"));
  }
  if (now.closeTab) hints.push(hint("Close tab", "global.close_tab"));
  if (now.newAgent) hints.push(hint("New agent", "global.new_agent"));
  if (now.newTopic) hints.push(hint("New topic", "global.new_topic"));
  if (now.newWorkspace) hints.push(hint("Add workspace", "global.new_workspace"));
  if (now.topics > 1) hints.push(hint("Topic", "global.prev_topic", "global.next_topic"));
  if (now.workspaces > 1) {
    hints.push(hint("Workspace", "global.prev_workspace", "global.next_workspace"));
  }
  if (now.sidebar) hints.push(hint("Sidebar or pane", "global.toggle_sidebar"));
  if (now.settings) hints.push(hint("Settings", "global.settings"));
  return hints;
}

/** The two that are worth seeing wherever you are, however little room there is. */
export function pinnedHints(requests: number): Hint[] {
  const commands = hint("Commands", "global.palette");
  return requests > 0 ? [hint("Next request", "global.next_request"), commands] : [commands];
}

let entries: HintEntry[] = [];
const subscribers = new Set<() => void>();

function publish(next: HintEntry[]) {
  entries = next;
  for (const notify of subscribers) notify();
}

/** Says which keys work here for as long as this is rendered, or none with null. Cheap enough
 * to call on every render: nothing happens unless the hints themselves changed. */
export function useHints(place: HintPlace, hints: Hint[] | null) {
  const key = hints && JSON.stringify(hints);
  useLayoutEffect(() => {
    if (!hints) return;
    const entry = { place, hints };
    publish([...entries, entry]);
    return () => publish(entries.filter((other) => other !== entry));
  }, [place, key]);
}

export function useHintEntries(): HintEntry[] {
  return useSyncExternalStore(
    (notify) => {
      subscribers.add(notify);
      return () => subscribers.delete(notify);
    },
    () => entries,
  );
}
