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
  | "reconnect"
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
  /** A device isn't connected, so trying again now could bring it back. */
  away?: boolean;
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
  const devices: ActionItem<AppItemId>[] = [];
  if (now.away) {
    devices.push({
      id: "reconnect",
      label: "Reconnect",
      inPalette: "Reconnect to offline devices",
      key: "global.reconnect",
    });
  }
  devices.push(
    {
      id: "add-device",
      label: "Add a device",
      inPalette: "Add a device",
      detail: "From another machine",
      key: "global.add_device",
    },
  );
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

export type DiffItemId =
  | "diff-scope"
  | "diff-search"
  | "diff-jump"
  | "diff-fold"
  | "diff-viewed"
  | "diff-copy"
  | "diff-stage-hunk"
  | "diff-stage-file"
  | "diff-stage-all"
  | "diff-discard-hunk"
  | "diff-discard-file"
  | "diff-commit"
  | "diff-amend"
  | "diff-fold-all"
  | "diff-files"
  | "diff-wrap";

/** What a diff tab offers, in its menu and the palette. A file's items act on the file at the
 * top of the view. */
export function diffItems(diff: {
  /** What switching scope would show. */
  switchTo: "uncommitted" | "branch";
  filesShown: boolean;
  wraps: boolean;
  /** Whether any file is unfolded, which folding every file would fold. */
  anyOpen: boolean;
  file?: { path: string; folded: boolean; viewed: boolean };
  /** Only uncommitted changes can be staged and committed. */
  git?: {
    /** The file at the top: whether it's the staged half, and whether its hunks go apart. */
    file?: { staged: boolean; hunks: boolean };
    anyStaged: boolean;
    anyUnstaged: boolean;
  };
}): ActionItem<DiffItemId>[][] {
  const scope = diff.switchTo === "branch" ? "the whole branch" : "uncommitted changes";
  const files = diff.filesShown ? "Hide the list of files" : "Show the list of files";
  const wrap = diff.wraps ? "Stop wrapping long lines" : "Wrap long lines";
  const all = diff.anyOpen ? "Fold every file" : "Unfold every file";
  const groups: ActionItem<DiffItemId>[][] = [
    [
      { id: "diff-scope", label: `Show ${scope}`, inPalette: `Show ${scope}`, detail: "Diff", key: "diff.scope" },
      { id: "diff-search", label: "Search", inPalette: "Search the diff", detail: "Diff", key: "diff.search" },
      { id: "diff-jump", label: "Go to a file", inPalette: "Go to a file in the diff", detail: "Diff", key: "diff.jump_file" },
    ],
  ];
  const { file } = diff;
  if (file) {
    const fold = file.folded ? "Unfold" : "Fold";
    const viewed = file.viewed ? "Mark not viewed" : "Mark viewed";
    groups.push([
      { id: "diff-fold", label: `${fold} this file`, inPalette: `${fold} ${file.path}`, detail: "Diff", key: "diff.toggle_file" },
      { id: "diff-viewed", label: viewed, inPalette: `${viewed}: ${file.path}`, detail: "Diff", key: "diff.viewed" },
      { id: "diff-copy", label: "Copy its path", inPalette: `Copy the path of ${file.path}`, detail: "Diff", key: "diff.copy_path" },
    ]);
  }
  const { git } = diff;
  if (git) {
    const items: ActionItem<DiffItemId>[] = [];
    const where = file ? `: ${file.path}` : "";
    if (git.file) {
      const verb = git.file.staged ? "Unstage" : "Stage";
      if (git.file.hunks) {
        items.push({ id: "diff-stage-hunk", label: `${verb} this hunk`, inPalette: `${verb} the hunk at the top${where}`, detail: "Diff", key: "diff.stage_hunk" });
      }
      items.push({ id: "diff-stage-file", label: `${verb} this file`, inPalette: `${verb} ${file?.path ?? "the file"}`, detail: "Diff", key: "diff.stage_file" });
    }
    if (git.anyUnstaged || git.anyStaged) {
      const all = git.anyUnstaged ? "Stage everything" : "Unstage everything";
      items.push({ id: "diff-stage-all", label: all, inPalette: all, detail: "Diff", key: "diff.stage_all" });
    }
    items.push(
      { id: "diff-commit", label: "Commit", inPalette: "Commit what's staged", detail: "Diff", key: "diff.commit" },
      { id: "diff-amend", label: "Amend the last commit", inPalette: "Amend the last commit", detail: "Diff", key: "diff.amend" },
    );
    groups.push(items);
    if (git.file && !git.file.staged) {
      const discard: ActionItem<DiffItemId>[] = [];
      if (git.file.hunks) {
        discard.push({ id: "diff-discard-hunk", label: "Discard this hunk", inPalette: `Discard the hunk at the top${where}`, detail: "Diff", key: "diff.discard_hunk" });
      }
      discard.push({ id: "diff-discard-file", label: "Discard this file", inPalette: `Discard the changes to ${file?.path ?? "the file"}`, detail: "Diff", key: "diff.discard_file" });
      groups.push(discard);
    }
  }
  groups.push([
    { id: "diff-fold-all", label: all, inPalette: all, detail: "Diff", key: "diff.toggle_all" },
    { id: "diff-files", label: files, inPalette: files, detail: "Diff", key: "diff.toggle_files" },
    { id: "diff-wrap", label: wrap, inPalette: wrap, detail: "Diff", key: "diff.toggle_wrap" },
  ]);
  return groups;
}
