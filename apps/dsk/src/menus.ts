// What can be done to a topic, and in the app as a whole, described once. The palette, the
// menus and a right click all read this, so none of them can offer something the others don't.

import type { ActionId } from "./keybinds";

export interface ActionItem<Id extends string = string> {
  id: Id;
  /** In a menu, where what it acts on is already clear. */
  label: string;
  /** In the palette, which has to say. */
  inPalette: string;
  detail?: string;
  /** The action whose key does the same. */
  key: ActionId;
}

/** An item with what runs it. */
export interface Runnable extends ActionItem {
  run: () => void;
}

export type TopicItemId =
  | "rename-topic"
  | "toggle-unread"
  | "move-topic"
  | "topic-mcp"
  | "import-conversation"
  | "archive-topic";

/** What a topic on a reachable device offers, in the groups a menu shows them in. Archiving
 * comes last and on its own, away from everything that's safe to press. */
export function topicItems(topic: {
  name: string;
  /** The branch of its worktree, if it has one. */
  branch?: string;
  /** What marking it would do, and to how many tabs. Nothing with none. */
  toggle: { unread: boolean; count: number };
  /** Its new tab is showing, where importing has a key of its own. */
  onNewTab: boolean;
}): ActionItem<TopicItemId>[][] {
  const { name, toggle } = topic;
  const state = toggle.unread ? "unread" : "read";
  const organize: ActionItem<TopicItemId>[] = [
    { id: "rename-topic", label: "Rename", inPalette: `Rename ${name}`, detail: "Topic", key: "sidebar.rename" },
  ];
  if (toggle.count > 0) {
    organize.push({
      id: "toggle-unread",
      label: `Mark ${state}`,
      inPalette: `Mark ${name} ${state}`,
      detail: "Topic",
      key: "sidebar.toggle_unread",
    });
  }
  organize.push({
    id: "move-topic",
    label: "Move to another folder",
    inPalette: `Move ${name} to another folder`,
    detail: topic.branch,
    key: "global.move_topic",
  });
  return [
    organize,
    [
      {
        id: "topic-mcp",
        label: "MCP servers",
        inPalette: `MCP servers for ${name}`,
        detail: "Topic",
        key: "global.topic_mcp",
      },
      {
        id: "import-conversation",
        label: "Import a conversation",
        inPalette: `Import a conversation into ${name}`,
        detail: "Claude Code or Codex",
        key: topic.onNewTab ? "new_tab.import" : "global.import_conversation",
      },
    ],
    [
      { id: "archive-topic", label: "Archive", inPalette: `Archive ${name}`, detail: "Topic", key: "sidebar.archive" },
    ],
  ];
}

export type AppItemId =
  | "next"
  | "mark-all-read"
  | "archived-topics"
  | "add-device"
  | "pair-machine";

/** What isn't about one topic. Only what would do something right now is there, and adding a
 * device always is, so there's never nothing. */
export function appItems(now: {
  /** Requests waiting on reachable devices. */
  requests: number;
  /** Topics with something unread. */
  unread: number;
  /** A device is reachable, so its archive can be asked for. */
  online: boolean;
  /** This machine's own device, which is where pairing starts. */
  local?: string;
}): ActionItem<AppItemId>[][] {
  const topics: ActionItem<AppItemId>[] = [];
  if (now.requests > 0) {
    topics.push({ id: "next", label: "Next request", inPalette: "Next request", key: "global.next_request" });
  }
  if (now.unread > 0) {
    topics.push({
      id: "mark-all-read",
      label: "Mark every topic read",
      inPalette: "Mark every topic read",
      key: "global.mark_all_read",
    });
  }
  if (now.online) {
    topics.push({
      id: "archived-topics",
      label: "Restore an archived topic",
      inPalette: "Restore an archived topic",
      key: "global.archived_topics",
    });
  }
  const devices: ActionItem<AppItemId>[] = [
    {
      id: "add-device",
      label: "Add a device",
      inPalette: "Add a device",
      detail: "From another machine",
      key: "global.add_device",
    },
  ];
  if (now.local !== undefined) {
    devices.push({
      id: "pair-machine",
      label: "Pair another machine",
      inPalette: `Pair another machine with ${now.local}`,
      key: "global.pair_machine",
    });
  }
  return [topics, devices];
}

/** Gives every item what runs it. The record has to cover every ID, so an item can't be added
 * without something to run. Groups left with nothing are dropped. */
export function withRuns<Id extends string>(
  groups: ActionItem<Id>[][],
  runs: Record<Id, () => void>,
): Runnable[][] {
  return groups
    .filter((group) => group.length > 0)
    .map((group) => group.map((item) => ({ ...item, run: runs[item.id] })));
}
