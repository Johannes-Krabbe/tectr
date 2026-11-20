// What can be done to a topic, in the app as a whole and in a diff, described once. The menus
// and a right click both read this, so neither can offer something the other doesn't.

import type { ActionId } from "./keybinds";

export interface ActionItem<Id extends string = string> {
  id: Id;
  label: string;
  /** The action whose key does the same, for the few that have one. */
  key?: ActionId;
}

/** An item with what runs it, or with a few to choose from instead, like where to move. */
export interface Runnable extends ActionItem {
  run: () => void;
  items?: { id: string; label: string; run: () => void }[];
}

export type TopicItemId =
  | "rename-topic"
  | "toggle-unread"
  | "move-topic"
  | "open-workspace"
  | "import-conversation"
  | "archive-topic";

/** What a topic on a reachable device offers, in the groups a menu shows them in. Archiving
 * comes last and on its own, away from everything that's safe to press. */
export function topicItems(topic: {
  /** What marking it would do, and to how many tabs. Nothing with none. */
  toggle: { unread: boolean; count: number };
}): ActionItem<TopicItemId>[][] {
  const { toggle } = topic;
  const organize: ActionItem<TopicItemId>[] = [{ id: "rename-topic", label: "Rename", key: "sidebar.rename" }];
  if (toggle.count > 0) {
    const state = toggle.unread ? "unread" : "read";
    organize.push({ id: "toggle-unread", label: `Mark ${state}`, key: "sidebar.toggle_unread" });
  }
  organize.push({ id: "move-topic", label: "Move to another folder" });
  return [
    organize,
    [
      { id: "open-workspace", label: "Open its workspace" },
      { id: "import-conversation", label: "Import a conversation" },
    ],
    [{ id: "archive-topic", label: "Archive", key: "sidebar.archive" }],
  ];
}

export type AppItemId = "next" | "mark-all-read";

/** What's about every topic at once. Only what would do something right now is there, so with
 * nothing waiting or unread there's nothing, and the menu isn't shown. Devices and pairing are
 * in Settings, and archived topics on their workspace's page. */
export function appItems(now: {
  /** Requests waiting on reachable devices. */
  requests: number;
  /** Topics with something unread. */
  unread: number;
}): ActionItem<AppItemId>[][] {
  const topics: ActionItem<AppItemId>[] = [];
  if (now.requests > 0) topics.push({ id: "next", label: "Next request", key: "global.next_request" });
  if (now.unread > 0) topics.push({ id: "mark-all-read", label: "Mark every topic read" });
  return [topics];
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

/** What a diff tab offers, in its menu. A file's items act on the file at the top of the
 * view. */
export function diffItems(diff: {
  /** What switching scope would show. */
  switchTo: "uncommitted" | "branch";
  filesShown: boolean;
  wraps: boolean;
  /** Whether any file is unfolded, which folding every file would fold. */
  anyOpen: boolean;
  file?: { folded: boolean; viewed: boolean };
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
      { id: "diff-scope", label: `Show ${scope}` },
      { id: "diff-search", label: "Search", key: "diff.search" },
      { id: "diff-jump", label: "Go to a file", key: "diff.jump_file" },
    ],
  ];
  const { file } = diff;
  if (file) {
    const fold = file.folded ? "Unfold" : "Fold";
    const viewed = file.viewed ? "Mark not viewed" : "Mark viewed";
    groups.push([
      { id: "diff-fold", label: `${fold} this file`, key: "diff.toggle_file" },
      { id: "diff-viewed", label: viewed, key: "diff.viewed" },
      { id: "diff-copy", label: "Copy its path" },
    ]);
  }
  const { git } = diff;
  if (git) {
    const items: ActionItem<DiffItemId>[] = [];
    if (git.file) {
      const verb = git.file.staged ? "Unstage" : "Stage";
      if (git.file.hunks) {
        items.push({ id: "diff-stage-hunk", label: `${verb} this hunk`, key: "diff.stage_hunk" });
      }
      items.push({ id: "diff-stage-file", label: `${verb} this file`, key: "diff.stage_file" });
    }
    if (git.anyUnstaged || git.anyStaged) {
      const all = git.anyUnstaged ? "Stage everything" : "Unstage everything";
      items.push({ id: "diff-stage-all", label: all });
    }
    items.push(
      { id: "diff-commit", label: "Commit", key: "diff.commit" },
      { id: "diff-amend", label: "Amend the last commit" },
    );
    groups.push(items);
    if (git.file && !git.file.staged) {
      const discard: ActionItem<DiffItemId>[] = [];
      if (git.file.hunks) {
        discard.push({ id: "diff-discard-hunk", label: "Discard this hunk" });
      }
      discard.push({ id: "diff-discard-file", label: "Discard this file" });
      groups.push(discard);
    }
  }
  groups.push([
    { id: "diff-fold-all", label: all },
    { id: "diff-files", label: files },
    { id: "diff-wrap", label: wrap },
  ]);
  return groups;
}
