import { invoke } from "@tauri-apps/api/core";
import {
  memo,
  type RefObject,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { motionAllowed } from "./appearance";
import {
  currentIndex,
  type DiffRow,
  fileGroups,
  fileKey,
  fileRows,
  type FileRows,
  findMatches,
  fingerprint,
  firstMatchFrom,
  hunkAt,
  hunksApart,
  languageOf,
  markSpans,
  nextStop,
  otherScope,
  parseViewed,
  pathsOf,
  spanLines,
  splitPath,
  STATUS_LETTER,
  STATUS_NAME,
  totals,
} from "./diff";
import {
  type Command,
  type Diff,
  type DiffContent,
  type FileDiff,
  errorText,
  request,
  type Topic,
} from "./engine";
import {
  act,
  type ActionId,
  diffShowsFiles,
  diffWraps,
  isTextField,
  keysLabel,
  notify,
  titled,
  toggleDiffFiles,
  toggleDiffWrap,
  useSettings,
} from "./keybinds";
import { inPane } from "./keys";
import type { SyntaxSpan } from "./markdown";
import { diffItems, type Runnable, withRuns } from "./menus";
import { topicWorktree } from "./model";
import { newScroller } from "./motion";
import { type PaletteItem, useScreenPaletteItems } from "./Palette";
import type { SheetRequest } from "./Sheets";
import { hint, useHints } from "./shortcuts";
import { Icon } from "./ui/Icon";
import { MenuButton } from "./ui/Menu";
import { Keys } from "./ui/StatusIcon";
import { useVim, VimModeLabel } from "./VimBox";

const SCROLL_STEP = 90;
/** A guess at a row's height, so files far off screen take about their room unrendered. */
const ROW_GUESS_PX = 20;

const SCOPES: { kind: "uncommitted" | "branch"; label: string }[] = [
  { kind: "uncommitted", label: "Uncommitted" },
  { kind: "branch", label: "Branch" },
];

const COMMIT_HINTS = [
  hint("Commit", "commit.send"),
  hint("Amend or not", "commit.toggle_amend"),
  hint("Close", "commit.cancel"),
];

const SEARCH_HINTS = [
  hint("Go to match", "diff_search.confirm"),
  hint("Stop searching", "diff_search.cancel"),
];

/** What was last read, with the scope and revision it was asked for, so a switch shows
 * nothing stale and a failure is asked again once something changes. */
type Loaded =
  | { scope: string; asked: number; content: DiffContent; error?: undefined }
  | { scope: string; asked: number; content?: undefined; error: string };

/** What this client is looking at in a tab. Only the shown tab is mounted, so it's kept here
 * while you look at another, and lost on restart like the rest of a view. */
interface View {
  /** Files folded by hand, by `fileKey`. */
  folded: Set<string>;
  /** Viewed files unfolded by hand, which viewing would otherwise fold. */
  unfolded: Set<string>;
  scrollTop: number;
  query: string;
  match: number | null;
  /** The commit message being written. */
  message: string;
}

const views = new Map<string, View>();

function viewOf(id: string): View {
  let view = views.get(id);
  if (!view) {
    view = {
      folded: new Set(),
      unfolded: new Set(),
      scrollTop: 0,
      query: "",
      match: null,
      message: "",
    };
    views.set(id, view);
  }
  return view;
}

const viewedKey = (id: string) => `tectr.diff.viewed.${id}`;

/** Forgets what a closed tab kept on this client. */
export function forgetDiffView(id: string) {
  views.delete(id);
  localStorage.removeItem(viewedKey(id));
}

/** Where to go once the next render has laid out what it changed. */
type Jump = { file: number } | { match: number };

/** What changed in a topic's folder, kept current by the daemon. The file list on the left
 * jumps within one scroll of every file. */
export function DiffPane({
  machineId,
  topic,
  diff,
  online,
  onPicker,
  onConfirm,
}: {
  machineId: string;
  topic: Topic;
  diff: Diff;
  online: boolean;
  /** Opens a list to choose from, as the palette does. */
  onPicker: (picker: { placeholder: string; empty: string; items: PaletteItem[] }) => void;
  /** Asks before something that can't be undone. */
  onConfirm: (sheet: SheetRequest) => void;
}) {
  useSettings();
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  // What went wrong with something just pressed, which leaves the diff on screen.
  const [failure, setFailure] = useState<string | null>(null);
  const [view, setViewState] = useState<View>(() => viewOf(diff.id));
  const [viewed, setViewedState] = useState(() =>
    parseViewed(localStorage.getItem(viewedKey(diff.id))),
  );
  const [searching, setSearching] = useState(false);
  const [current, setCurrent] = useState(0);
  // The hunk the keys act on, in the file at the top.
  const [currentHunk, setCurrentHunk] = useState<number | null>(null);
  // The commit box, open with whether it amends.
  const [committing, setCommitting] = useState<{ amend: boolean } | null>(null);
  const [commitError, setCommitError] = useState<string | null>(null);
  const [commitBusy, setCommitBusy] = useState(false);
  const messageBox = useRef<HTMLTextAreaElement>(null);
  const vim = useVim(messageBox);
  const body = useRef<HTMLDivElement>(null);
  const searchBox = useRef<HTMLInputElement>(null);
  const jump = useRef<Jump | null>(null);
  const restored = useRef(false);
  const [scroller] = useState(() => newScroller(() => body.current, motionAllowed));
  const scope = diff.scope.kind;
  const revision = diff.revision ?? 0;
  const showFiles = diffShowsFiles();
  const wraps = diffWraps();

  const setView = (change: (view: View) => Partial<View>) =>
    setViewState((previous) => {
      const next = { ...previous, ...change(previous) };
      views.set(diff.id, next);
      return next;
    });
  const setViewed = (next: Record<string, string>) => {
    setViewedState(next);
    localStorage.setItem(viewedKey(diff.id), JSON.stringify(next));
  };

  // Revisions only go up, so an answer older than the tab's revision is read again. A press
  // sent twice before its answer gets the first answer, which this catches too.
  const shown = loaded?.scope === scope ? loaded : null;
  const stale = !shown || (shown.content ? shown.content.revision : shown.asked) < revision;
  useEffect(() => {
    if (!online || !stale) return;
    let active = true;
    const asked = revision;
    request(machineId, { command: "read_diff", diff_id: diff.id })
      .then((output) => {
        if (active && output.output === "diff_content") {
          setLoaded({ scope, asked, content: output.content });
        }
      })
      .catch((reason) => {
        if (active) setLoaded({ scope, asked, error: errorText(reason) });
      });
    return () => {
      active = false;
    };
  }, [machineId, diff.id, scope, revision, online, stale]);

  const content = shown?.content;
  const groups = useMemo(
    () => (content ? fileGroups(content, diff.scope) : []),
    [content, diff.scope],
  );
  const files = useMemo(() => groups.flatMap((group) => group.files), [groups]);
  const rows = useMemo(
    () => files.map((file) => (file.body.kind === "text" ? fileRows(file.body.hunks) : null)),
    [files],
  );
  const prints = useMemo(() => files.map(fingerprint), [files]);
  const sum = totals(files);
  const isViewed = (index: number) => viewed[fileKey(files[index])] === prints[index];
  const isFolded = (index: number) => {
    const key = fileKey(files[index]);
    return view.folded.has(key) || (isViewed(index) && !view.unfolded.has(key));
  };
  const viewedCount = files.filter((_, index) => isViewed(index)).length;
  const anyOpen = files.some((_, index) => !isFolded(index));

  const matches = useMemo(
    () => findMatches(rows.map((r) => r?.rows ?? null), view.query),
    [rows, view.query],
  );
  const match = view.match !== null && view.match < matches.length ? view.match : null;
  // Each file's marks by row, so a file without any keeps its memo.
  const marks = useMemo(() => {
    const byFile = new Map<number, Map<number, Mark[]>>();
    matches.forEach((m, index) => {
      const fileMarks = byFile.get(m.file) ?? new Map<number, Mark[]>();
      byFile.set(m.file, fileMarks);
      const rowMarks = fileMarks.get(m.row) ?? [];
      fileMarks.set(m.row, rowMarks);
      rowMarks.push({ start: m.start, end: m.end, current: index === match });
    });
    return byFile;
  }, [matches, match]);

  const tops = (selector: string) => {
    const el = body.current;
    if (!el) return [];
    const start = el.getBoundingClientRect().top - el.scrollTop;
    return [...el.querySelectorAll<HTMLElement>(selector)].map(
      (node) => node.getBoundingClientRect().top - start,
    );
  };

  const go = (top: number, now = false) => {
    const el = body.current;
    if (!el) return;
    const clamped = Math.max(0, Math.min(top, el.scrollHeight - el.clientHeight));
    if (now) scroller.jump(clamped);
    else scroller.to(clamped);
  };

  // A jump waits for the render that folds or unfolds what it goes to.
  useLayoutEffect(() => {
    const target = jump.current;
    const el = body.current;
    if (!target || !el) return;
    jump.current = null;
    if ("file" in target) {
      const top = tops("[data-file]")[target.file];
      if (top !== undefined) go(top);
      return;
    }
    const found = matches[target.match];
    const row = found && el.querySelector<HTMLElement>(`[data-row="${found.file}:${found.row}"]`);
    if (!row) return;
    const top = row.getBoundingClientRect().top - el.getBoundingClientRect().top + el.scrollTop;
    // A third of the way down leaves room to read what's around it.
    go(top - el.clientHeight / 3);
  });

  // Back where you were when you come back to this tab.
  useLayoutEffect(() => {
    if (restored.current || !content || !body.current) return;
    restored.current = true;
    body.current.scrollTop = view.scrollTop;
  }, [content]);

  const saveFailed = (reason: unknown) => setFailure(errorText(reason));

  const switchScope = () => {
    if (!online) return;
    setFailure(null);
    request(machineId, {
      command: "set_diff_scope",
      diff_id: diff.id,
      scope: otherScope(diff.scope),
    }).catch(saveFailed);
  };

  const toggleFile = (index: number) => {
    const file = files[index];
    if (!file) return;
    const key = fileKey(file);
    const folding = !isFolded(index);
    setView((v) => {
      const folded = new Set(v.folded);
      const unfolded = new Set(v.unfolded);
      if (folding) {
        folded.add(key);
        unfolded.delete(key);
      } else {
        folded.delete(key);
        unfolded.add(key);
      }
      return { folded, unfolded };
    });
    // Folding from inside a file would leave the view somewhere further down.
    const top = tops("[data-file]")[index];
    if (folding && top !== undefined && top < (body.current?.scrollTop ?? 0)) {
      jump.current = { file: index };
    }
  };

  const toggleAll = () => {
    const keys = files.map(fileKey);
    setView(() =>
      anyOpen
        ? { folded: new Set(keys), unfolded: new Set() }
        : { folded: new Set(), unfolded: new Set(keys) },
    );
  };

  /** Viewing folds a file and moves on to the next, as reviewing goes. */
  const toggleViewed = (index: number) => {
    const file = files[index];
    if (!file) return;
    const key = fileKey(file);
    const next = { ...viewed };
    const viewing = !isViewed(index);
    if (viewing) next[key] = prints[index];
    else delete next[key];
    setViewed(next);
    setView((v) => {
      const folded = new Set(v.folded);
      const unfolded = new Set(v.unfolded);
      folded.delete(key);
      unfolded.delete(key);
      return { folded, unfolded };
    });
    if (viewing) jump.current = { file: Math.min(index + 1, files.length - 1) };
  };

  const copyPath = (index: number) => {
    const file = files[index];
    if (!file) return;
    invoke("write_clipboard", { text: file.path })
      .then(() => notify(`Copied ${file.path}`))
      .catch(saveFailed);
  };

  const jumpToFile = (index: number) => {
    if (!isFolded(index)) {
      const top = tops("[data-file]")[index];
      if (top !== undefined) go(top);
      return;
    }
    toggleFile(index);
    jump.current = { file: index };
  };

  const pickFile = () =>
    onPicker({
      placeholder: "Go to a file",
      empty: "No files",
      items: files.map((file, index) => ({
        id: `diff-file/${fileKey(file)}`,
        label: file.path,
        detail: [
          file.staged ? "Staged" : "",
          STATUS_NAME[file.status],
          `+${file.added} -${file.removed}`,
        ]
          .filter(Boolean)
          .join(", "),
        run: () => jumpToFile(index),
      })),
    });

  /** Shows a match, unfolding its file if it has to. */
  const reveal = (index: number | null) => {
    setView(() => ({ match: index }));
    if (index === null) return;
    const file = matches[index]?.file;
    if (file !== undefined && isFolded(file)) toggleFile(file);
    jump.current = { match: index };
  };

  const search = (query: string) => {
    const found = findMatches(
      rows.map((r) => r?.rows ?? null),
      query,
    );
    const index = firstMatchFrom(found, current);
    setView(() => ({ query, match: index }));
    if (index === null) return;
    const file = found[index].file;
    if (isFolded(file)) toggleFile(file);
    jump.current = { match: index };
  };

  /** Moves to the next file or hunk. A hunk stops below its file's header, which stays at
   * the top while its file scrolls. */
  const stepTo = (selector: string, step: 1 | -1) => {
    const header = body.current?.querySelector<HTMLElement>(".diff-file-head");
    const offset = selector === "[data-hunk]" ? (header?.offsetHeight ?? 0) : 0;
    const stops = tops(selector).map((top) => Math.max(0, top - offset));
    const top = nextStop(stops, scroller.headedTo(), step);
    if (top !== null) go(top);
  };

  /** Without a search, the match keys move between hunks, which is what they did before. */
  const stepMatch = (step: 1 | -1) => {
    if (!view.query) {
      stepTo("[data-hunk]", step);
      return;
    }
    if (matches.length === 0) return;
    const from = match ?? (step > 0 ? -1 : 0);
    reveal((from + step + matches.length) % matches.length);
  };

  const openSearch = () => {
    setSearching(true);
    requestAnimationFrame(() => {
      searchBox.current?.focus();
      searchBox.current?.select();
    });
  };
  const stopSearching = (keep: boolean) => {
    setSearching(false);
    if (!keep) setView(() => ({ query: "", match: null }));
    body.current?.focus();
  };

  // Only uncommitted changes have a staged side, so only they can be staged and committed.
  const canEdit = scope === "uncommitted" && online;
  const anyStaged = files.some((file) => file.staged);
  const anyUnstaged = files.some((file) => !file.staged);

  const send = (command: Command) => {
    setFailure(null);
    request(machineId, command).catch(saveFailed);
  };

  /** The hunk at the top of a file, which whole-file changes stand in for when its hunks
   * can't go apart. */
  const hunkOf = (index: number, hunk: number | null) => {
    const file = files[index];
    if (!file || hunk === null || file.body.kind !== "text") return null;
    return file.body.hunks[hunk] ?? null;
  };

  const stageFile = (index: number) => {
    const file = files[index];
    if (!file || !canEdit) return;
    send({ command: "stage_files", diff_id: diff.id, paths: pathsOf(file), stage: !file.staged });
  };

  const stageHunk = (index: number, hunkIndex: number | null) => {
    const file = files[index];
    if (!file || !canEdit) return;
    const hunk = hunkOf(index, hunkIndex);
    if (!hunksApart(file) || !hunk) {
      stageFile(index);
      return;
    }
    send({ command: "stage_hunk", diff_id: diff.id, path: file.path, hunk, stage: !file.staged });
  };

  const stageAll = () => {
    if (!canEdit) return;
    const stage = anyUnstaged;
    const paths = files.filter((file) => file.staged !== stage).flatMap(pathsOf);
    if (paths.length > 0) send({ command: "stage_files", diff_id: diff.id, paths, stage });
  };

  const discardFile = (index: number) => {
    const file = files[index];
    if (!file || !canEdit) return;
    if (file.staged) {
      setFailure("Unstage it first. Only what isn't staged can be discarded");
      return;
    }
    const untracked = file.status === "untracked";
    onConfirm({
      kind: "confirm",
      machineId,
      where: untracked ? `Delete ${file.path}?` : `Discard the changes to ${file.path}?`,
      action: untracked ? "Delete" : "Discard",
      command: { command: "discard_files", diff_id: diff.id, paths: pathsOf(file) },
    });
  };

  const discardHunk = (index: number, hunkIndex: number | null) => {
    const file = files[index];
    if (!file || !canEdit) return;
    const hunk = hunkOf(index, hunkIndex);
    if (file.staged || !hunksApart(file) || !hunk) {
      discardFile(index);
      return;
    }
    onConfirm({
      kind: "confirm",
      machineId,
      where: `Discard this hunk of ${file.path}?`,
      action: "Discard",
      command: { command: "discard_hunk", diff_id: diff.id, path: file.path, hunk },
    });
  };

  const openCommit = (amend: boolean) => {
    if (!canEdit) return;
    // Amending starts from the last message, unless something is already written.
    if (amend && !view.message.trim() && content?.head_message) {
      const message = content.head_message;
      setView(() => ({ message }));
    }
    setCommitError(null);
    setCommitting({ amend });
    requestAnimationFrame(() => messageBox.current?.focus());
  };

  const closeCommit = () => {
    setCommitting(null);
    body.current?.focus();
  };

  const commit = async () => {
    if (!committing || commitBusy) return;
    const message = view.message.trim();
    if (!committing.amend && !anyStaged) {
      setCommitError("Nothing is staged");
      return;
    }
    if (!committing.amend && !message) {
      setCommitError("A commit needs a message");
      return;
    }
    setCommitBusy(true);
    setCommitError(null);
    try {
      await request(machineId, {
        command: "commit",
        diff_id: diff.id,
        message,
        amend: committing.amend,
      });
      setView(() => ({ message: "" }));
      notify(committing.amend ? "Amended the last commit" : "Committed");
      closeCommit();
    } catch (reason) {
      setCommitError(errorText(reason));
    } finally {
      setCommitBusy(false);
    }
  };

  const currentFile = files[current];
  const items: Runnable[][] = withRuns(
    diffItems({
      switchTo: scope === "branch" ? "uncommitted" : "branch",
      filesShown: showFiles,
      wraps,
      anyOpen,
      file: currentFile && {
        path: currentFile.path,
        folded: isFolded(current),
        viewed: isViewed(current),
      },
      git: canEdit
        ? {
            file: currentFile && {
              staged: currentFile.staged ?? false,
              hunks: hunksApart(currentFile) && currentHunk !== null,
            },
            anyStaged,
            anyUnstaged,
          }
        : undefined,
    }),
    {
      "diff-scope": switchScope,
      "diff-search": openSearch,
      "diff-jump": pickFile,
      "diff-fold": () => toggleFile(current),
      "diff-viewed": () => toggleViewed(current),
      "diff-copy": () => copyPath(current),
      "diff-stage-hunk": () => stageHunk(current, currentHunk),
      "diff-stage-file": () => stageFile(current),
      "diff-stage-all": stageAll,
      "diff-discard-hunk": () => discardHunk(current, currentHunk),
      "diff-discard-file": () => discardFile(current),
      "diff-commit": () => openCommit(false),
      "diff-amend": () => openCommit(true),
      "diff-fold-all": toggleAll,
      "diff-files": () => void toggleDiffFiles().catch(saveFailed),
      "diff-wrap": () => void toggleDiffWrap().catch(saveFailed),
    },
  );
  useScreenPaletteItems(
    items.flat().map(({ id, inPalette, detail, key, run }) => ({
      id,
      label: inPalette,
      detail,
      keys: keysLabel(key),
      run,
    })),
  );
  useHints("pane", [
    hint("Scroll", "diff.up", "diff.down"),
    hint("Files", "diff.prev_file", "diff.next_file"),
    hint(view.query ? "Matches" : "Hunks", "diff.prev_match", "diff.next_match"),
    hint(currentFile && isFolded(current) ? "Unfold" : "Fold", "diff.toggle_file"),
    hint("Viewed", "diff.viewed"),
    ...(canEdit
      ? [
          hint(currentFile?.staged ? "Unstage hunk" : "Stage hunk", "diff.stage_hunk"),
          hint("Commit", "diff.commit"),
        ]
      : []),
    hint("Search", "diff.search"),
    hint("Go to file", "diff.jump_file"),
    hint("Scope", "diff.scope"),
    hint("Sidebar", "pane.back"),
  ]);
  useHints("typing", searching ? SEARCH_HINTS : committing ? COMMIT_HINTS : null);

  // The handler is added once, so it reaches the current state through a ref.
  const latest = {
    toggleFile,
    toggleViewed,
    toggleAll,
    copyPath,
    pickFile,
    openSearch,
    stepMatch,
    stepTo,
    switchScope,
    saveFailed,
    go,
    tops,
    current,
    currentHunk,
    stageHunk,
    stageFile,
    stageAll,
    discardHunk,
    discardFile,
    openCommit,
  };
  const actions = useRef(latest);
  actions.current = latest;
  const onToggle = useCallback((index: number) => actions.current.toggleFile(index), []);
  const onViewed = useCallback((index: number) => actions.current.toggleViewed(index), []);
  const onCopy = useCallback((index: number) => actions.current.copyPath(index), []);
  const onStageHunk = useCallback(
    (index: number, hunk: number) => actions.current.stageHunk(index, hunk),
    [],
  );
  const onStageFile = useCallback((index: number) => actions.current.stageFile(index), []);
  const onDiscardHunk = useCallback(
    (index: number, hunk: number) => actions.current.discardHunk(index, hunk),
    [],
  );
  const onDiscardFile = useCallback((index: number) => actions.current.discardFile(index), []);

  useEffect(() => {
    const now = () => actions.current;
    const scrollTo = (top: (el: HTMLElement) => number) => () => {
      const el = body.current;
      if (el) now().go(top(el));
    };
    const scrollBy = (amount: (el: HTMLElement) => number) =>
      scrollTo((el) => scroller.headedTo() + amount(el));
    const stepTo = (selector: string, step: 1 | -1) => () => now().stepTo(selector, step);
    const commands: [ActionId, () => void][] = [
      ["diff.up", scrollBy(() => -SCROLL_STEP)],
      ["diff.down", scrollBy(() => SCROLL_STEP)],
      ["diff.half_up", scrollBy((el) => -el.clientHeight / 2)],
      ["diff.half_down", scrollBy((el) => el.clientHeight / 2)],
      ["diff.page_up", scrollBy((el) => -el.clientHeight)],
      ["diff.page_down", scrollBy((el) => el.clientHeight)],
      ["diff.first", scrollTo(() => 0)],
      ["diff.last", scrollTo((el) => el.scrollHeight)],
      ["diff.next_file", stepTo("[data-file]", 1)],
      ["diff.prev_file", stepTo("[data-file]", -1)],
      ["diff.next_hunk", stepTo("[data-hunk]", 1)],
      ["diff.prev_hunk", stepTo("[data-hunk]", -1)],
      ["diff.toggle_file", () => now().toggleFile(now().current)],
      ["diff.toggle_all", () => now().toggleAll()],
      ["diff.viewed", () => now().toggleViewed(now().current)],
      ["diff.copy_path", () => now().copyPath(now().current)],
      ["diff.jump_file", () => now().pickFile()],
      ["diff.search", () => now().openSearch()],
      ["diff.next_match", () => now().stepMatch(1)],
      ["diff.prev_match", () => now().stepMatch(-1)],
      ["diff.scope", () => now().switchScope()],
      ["diff.toggle_files", () => void toggleDiffFiles().catch(now().saveFailed)],
      ["diff.toggle_wrap", () => void toggleDiffWrap().catch(now().saveFailed)],
      ["diff.stage_hunk", () => now().stageHunk(now().current, now().currentHunk)],
      ["diff.stage_file", () => now().stageFile(now().current)],
      ["diff.stage_all", () => now().stageAll()],
      ["diff.discard_hunk", () => now().discardHunk(now().current, now().currentHunk)],
      ["diff.discard_file", () => now().discardFile(now().current)],
      ["diff.commit", () => now().openCommit(false)],
      ["diff.amend", () => now().openCommit(true)],
    ];
    const onKey = (event: KeyboardEvent) => {
      // In the search box, Ctrl+D and the like edit the text.
      if (!inPane(event) || isTextField(event.target)) return;
      commands.some(([id, run]) => act(event, id, run));
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      scroller.stop();
    };
  }, []);

  // Lands with the keyboard, so single keys work straight away.
  useEffect(() => {
    const active = document.activeElement;
    if (!active || active === document.body) body.current?.focus();
  }, []);

  // Which file is at the top, read at most once a frame while scrolling.
  const reading = useRef(0);
  const onScroll = () => {
    const el = body.current;
    if (!el) return;
    viewOf(diff.id).scrollTop = el.scrollTop;
    if (reading.current) return;
    reading.current = requestAnimationFrame(() => {
      reading.current = 0;
      if (body.current) readCurrent(body.current);
    });
  };

  /** Which file and hunk are at the top. A hunk counts once its header is below its file's
   * header, which stays at the top while its file scrolls. */
  const readCurrent = (el: HTMLElement) => {
    const file = currentIndex(tops("[data-file]"), el.scrollTop);
    const header = el.querySelector<HTMLElement>(".diff-file-head")?.offsetHeight ?? 0;
    const hunkTops = tops("[data-hunk]");
    const hunks = [...el.querySelectorAll<HTMLElement>("[data-hunk]")].map((node, at) => {
      const [f, h] = (node.dataset.hunk ?? "").split(":").map(Number);
      return { file: f, hunk: h, top: hunkTops[at] - header };
    });
    setCurrent(file);
    setCurrentHunk(hunkAt(hunks, file, el.scrollTop));
  };

  // What's at the top changes when files move between staged and not, without a scroll.
  useLayoutEffect(() => {
    if (body.current) readCurrent(body.current);
  }, [content]);

  const worktree = topicWorktree(topic);
  const against = scope === "branch" && content?.base ? `against ${content.base}` : null;
  let index = 0;

  return (
    <div className="pane diff">
      <div className="diff-head">
        <div className="diff-scopes" role="radiogroup" aria-label="Compare">
          {SCOPES.map(({ kind, label }) => (
            <button
              key={kind}
              role="radio"
              aria-checked={scope === kind}
              className={`diff-scope ${scope === kind ? "chosen" : ""}`}
              tabIndex={-1}
              disabled={!online}
              title={scope === kind ? undefined : titled("Switch", "diff.scope")}
              onClick={() => scope !== kind && switchScope()}
            >
              {label}
            </button>
          ))}
        </div>
        <span className="meta">{[worktree?.branch, against].filter(Boolean).join(" ")}</span>
        <span className="diff-sum">
          {content && files.length > 0 && (
            <>
              <span className="meta">
                {viewedCount > 0 ? `${viewedCount} of ` : ""}
                {files.length} {files.length === 1 ? "file" : "files"}
                {viewedCount > 0 ? " viewed" : ""}
              </span>
              <span className="diff-added">+{sum.added}</span>
              <span className="diff-removed">-{sum.removed}</span>
            </>
          )}
        </span>
        {canEdit && (
          <button
            className="button small"
            tabIndex={-1}
            disabled={!anyStaged}
            title={titled("Commit what's staged", "diff.commit")}
            onClick={() => openCommit(false)}
          >
            Commit
          </button>
        )}
        <button
          className="icon-button"
          tabIndex={-1}
          title={titled("Search", "diff.search")}
          aria-label="Search"
          onClick={openSearch}
        >
          <Icon name="search" />
        </button>
        <MenuButton
          label="Diff actions"
          className="icon-button"
          groups={items.map((group) =>
            group.map(({ id, label, key, run }) => ({ id, label, keys: keysLabel(key), run })),
          )}
        >
          <Icon name="more" />
        </MenuButton>
      </div>
      {(searching || view.query) && (
        <div className="diff-search">
          <Icon name="search" />
          <input
            ref={searchBox}
            value={view.query}
            placeholder="Search"
            aria-label="Search the diff"
            spellCheck={false}
            onFocus={() => setSearching(true)}
            onBlur={() => setSearching(false)}
            onChange={(event) => search(event.target.value)}
            onKeyDown={(event) => {
              if (event.nativeEvent.isComposing) return;
              if (act(event, "diff_search.confirm", () => stopSearching(true))) return;
              act(event, "diff_search.cancel", () => stopSearching(false));
            }}
          />
          <span className="meta">
            {view.query
              ? matches.length === 0
                ? "No matches"
                : `${match === null ? "-" : match + 1} of ${matches.length}`
              : ""}
          </span>
        </div>
      )}
      {committing && (
        <div className="diff-commit">
          <div className="diff-commit-box">
            <textarea
              ref={messageBox}
              className="diff-commit-message"
              value={view.message}
              rows={3}
              placeholder={committing.amend ? "Message, or leave as it was" : "Commit message"}
              aria-label="Commit message"
              spellCheck={false}
              onChange={(event) => setView(() => ({ message: event.target.value }))}
              {...vim.props}
              onKeyDown={(event) => {
                if (event.nativeEvent.isComposing) return;
                if (vim.onKeyDown(event)) return;
                if (act(event, "commit.send", () => void commit())) return;
                if (act(event, "commit.toggle_amend", () => setCommitting({ amend: !committing.amend }))) {
                  return;
                }
                act(event, "commit.cancel", closeCommit);
              }}
            />
            <VimModeLabel mode={vim.shown} />
          </div>
          <div className="diff-commit-foot">
            {commitError && <span className="error">{commitError}</span>}
            <label className="diff-commit-amend" title={titled("Amend or not", "commit.toggle_amend")}>
              <input
                type="checkbox"
                tabIndex={-1}
                checked={committing.amend}
                onChange={(event) => setCommitting({ amend: event.target.checked })}
              />
              Amend the last commit
            </label>
            <button className="button" tabIndex={-1} onClick={closeCommit}>
              Cancel
              <Keys action="commit.cancel" />
            </button>
            <button
              className="button primary"
              tabIndex={-1}
              disabled={commitBusy}
              onClick={() => void commit()}
            >
              {committing.amend ? "Amend" : "Commit"}
              <Keys action="commit.send" />
            </button>
          </div>
        </div>
      )}
      {failure && <p className="error main-error">{failure}</p>}
      {shown?.error !== undefined ? (
        <div className="pane empty">
          <p className="error">{shown.error}</p>
        </div>
      ) : content && files.length === 0 ? (
        <div className="pane empty">
          <p>{scope === "branch" ? "Nothing changed on this branch" : "No uncommitted changes"}</p>
        </div>
      ) : (
        <div className={`diff-view ${wraps ? "wrap" : ""}`}>
          {showFiles && content && (
            <nav className="diff-files" aria-label="Files">
              {groups.map((group) => (
                <div key={group.title || "files"} className="diff-file-group">
                  {group.title && <h3 className="diff-group-title">{group.title}</h3>}
                  {group.files.map((file) => {
                    const at = index++;
                    const { folder, name } = splitPath(file.path);
                    return (
                      <button
                        key={fileKey(file)}
                        className={`diff-file-link ${at === current ? "current" : ""} ${isViewed(at) ? "viewed" : ""}`}
                        tabIndex={-1}
                        title={file.path}
                        onClick={() => jumpToFile(at)}
                      >
                        <span className={`diff-status ${file.status}`} title={STATUS_NAME[file.status]}>
                          {STATUS_LETTER[file.status]}
                        </span>
                        <span className="diff-file-name">{name}</span>
                        <span className="diff-file-folder">{folder}</span>
                        {isViewed(at) && <Icon name="viewed" />}
                      </button>
                    );
                  })}
                </div>
              ))}
            </nav>
          )}
          <div
            ref={body}
            className="diff-body"
            tabIndex={0}
            onScroll={onScroll}
            // Your own scrolling always wins over easing.
            onWheel={() => scroller.stop()}
          >
            {(() => {
              let at = 0;
              return groups.map((group) => (
                <section key={group.title || "files"}>
                  {group.title && <h3 className="diff-group-title wide">{group.title}</h3>}
                  {group.files.map((file) => {
                    const fileIndex = at++;
                    return (
                      <FileView
                        key={fileKey(file)}
                        file={file}
                        index={fileIndex}
                        rows={rows[fileIndex]}
                        folded={isFolded(fileIndex)}
                        viewed={isViewed(fileIndex)}
                        marks={marks.get(fileIndex)}
                        root={body}
                        canEdit={canEdit}
                        currentHunk={fileIndex === current ? currentHunk : null}
                        onToggle={onToggle}
                        onViewed={onViewed}
                        onCopy={onCopy}
                        onStageHunk={onStageHunk}
                        onStageFile={onStageFile}
                        onDiscardHunk={onDiscardHunk}
                        onDiscardFile={onDiscardFile}
                      />
                    );
                  })}
                </section>
              ));
            })()}
          </div>
        </div>
      )}
    </div>
  );
}

interface Mark {
  start: number;
  end: number;
  current: boolean;
}

const FileView = memo(function FileView({
  file,
  index,
  rows,
  folded,
  viewed,
  marks,
  root,
  canEdit,
  currentHunk,
  onToggle,
  onViewed,
  onCopy,
  onStageHunk,
  onStageFile,
  onDiscardHunk,
  onDiscardFile,
}: {
  file: FileDiff;
  index: number;
  rows: FileRows | null;
  folded: boolean;
  viewed: boolean;
  marks: Map<number, Mark[]> | undefined;
  root: RefObject<HTMLDivElement | null>;
  /** Whether it can be staged and discarded. */
  canEdit: boolean;
  /** The hunk the keys act on, when this is the file at the top. */
  currentHunk: number | null;
  onToggle: (index: number) => void;
  onViewed: (index: number) => void;
  onCopy: (index: number) => void;
  onStageHunk: (index: number, hunk: number) => void;
  onStageFile: (index: number) => void;
  onDiscardHunk: (index: number, hunk: number) => void;
  onDiscardFile: (index: number) => void;
}) {
  const section = useRef<HTMLElement>(null);
  const near = useNear(section, root) && !folded;
  const language = languageOf(file.path);
  const oldSpans = useHighlight(near ? (rows?.oldText ?? null) : null, language);
  const newSpans = useHighlight(near ? (rows?.newText ?? null) : null, language);
  const { folder, name } = splitPath(file.path);
  const height = folded ? 1 : (rows?.rows.length ?? 1) + 1;

  return (
    <section
      ref={section}
      className={`diff-file ${viewed ? "viewed" : ""}`}
      data-file
      style={{ containIntrinsicSize: `auto ${height * ROW_GUESS_PX}px` }}
    >
      <header className="diff-file-head" onClick={() => onToggle(index)}>
        <span className="diff-fold" aria-hidden>
          <Icon name={folded ? "unfold" : "fold"} />
        </span>
        <span className={`diff-status ${file.status}`} title={STATUS_NAME[file.status]}>
          {STATUS_LETTER[file.status]}
        </span>
        <span className="diff-file-path" title={file.path}>
          {file.old_path && (
            <span className="diff-file-folder">
              {file.old_path} {"->"}{" "}
            </span>
          )}
          <span className="diff-file-folder">{folder}</span>
          <span className="diff-file-name">{name}</span>
        </span>
        <button
          className="icon-button diff-file-action"
          tabIndex={-1}
          title={titled("Copy the path", "diff.copy_path")}
          aria-label={`Copy the path of ${file.path}`}
          onClick={(event) => {
            event.stopPropagation();
            onCopy(index);
          }}
        >
          <Icon name="copy" />
        </button>
        {canEdit && (
          <span className="diff-file-git">
            <button
              className="button small"
              tabIndex={-1}
              title={titled(file.staged ? "Unstage the file" : "Stage the file", "diff.stage_file")}
              onClick={(event) => {
                event.stopPropagation();
                onStageFile(index);
              }}
            >
              {file.staged ? "Unstage" : "Stage"}
            </button>
            {!file.staged && (
              <button
                className="button small"
                tabIndex={-1}
                title={titled("Discard the file's changes", "diff.discard_file")}
                onClick={(event) => {
                  event.stopPropagation();
                  onDiscardFile(index);
                }}
              >
                {file.status === "untracked" ? "Delete" : "Discard"}
              </button>
            )}
          </span>
        )}
        <span className="diff-added diff-file-counts">+{file.added}</span>
        <span className="diff-removed">-{file.removed}</span>
        <button
          className={`diff-viewed ${viewed ? "on" : ""}`}
          tabIndex={-1}
          aria-pressed={viewed}
          title={titled(viewed ? "Mark not viewed" : "Mark viewed", "diff.viewed")}
          onClick={(event) => {
            event.stopPropagation();
            onViewed(index);
          }}
        >
          <span className="diff-check">{viewed && <Icon name="viewed" />}</span>
          Viewed
        </button>
      </header>
      {folded ? null : rows ? (
        rows.rows.length > 0 ? (
          <div className="diff-lines">
            <table className="diff-table">
              <tbody>
                {rows.rows.map((row, at) =>
                  row.kind === "hunk" ? (
                    <tr
                      key={at}
                      className={`diff-hunk ${row.hunk === currentHunk ? "current" : ""}`}
                      data-hunk={`${index}:${row.hunk}`}
                    >
                      <td colSpan={3}>
                        <div className="diff-hunk-line">
                          <span className="diff-hunk-text" title={row.context || undefined}>
                            {row.range}
                            {row.context && <span className="diff-hunk-context"> {row.context}</span>}
                          </span>
                          {canEdit && hunksApart(file) && (
                            <span className="diff-hunk-git">
                              <button
                                className="button small"
                                tabIndex={-1}
                                title={titled(
                                  file.staged ? "Unstage the hunk" : "Stage the hunk",
                                  "diff.stage_hunk",
                                )}
                                onClick={() => onStageHunk(index, row.hunk)}
                              >
                                {file.staged ? "Unstage" : "Stage"}
                              </button>
                              {!file.staged && (
                                <button
                                  className="button small"
                                  tabIndex={-1}
                                  title={titled("Discard the hunk", "diff.discard_hunk")}
                                  onClick={() => onDiscardHunk(index, row.hunk)}
                                >
                                  Discard
                                </button>
                              )}
                            </span>
                          )}
                        </div>
                      </td>
                    </tr>
                  ) : (
                    <Row
                      key={at}
                      id={`${index}:${at}`}
                      row={row}
                      oldSpans={oldSpans}
                      newSpans={newSpans}
                      marks={marks?.get(at)}
                    />
                  ),
                )}
              </tbody>
            </table>
          </div>
        ) : (
          <p className="diff-note">{file.status === "modified" ? "Only its mode changed" : "Empty"}</p>
        )
      ) : (
        <p className="diff-note">{file.body.kind === "binary" ? "Binary file" : "Too large to show"}</p>
      )}
    </section>
  );
});

function Row({
  id,
  row,
  oldSpans,
  newSpans,
  marks,
}: {
  id: string;
  row: DiffRow;
  oldSpans: SyntaxSpan[][] | null;
  newSpans: SyntaxSpan[][] | null;
  marks: Mark[] | undefined;
}) {
  // Hunk headers are drawn by their file, which has what they need.
  if (row.kind === "hunk") return null;
  if (row.kind === "no_newline") {
    return (
      <tr className="diff-meta">
        <td />
        <td />
        <td>No newline at end of file</td>
      </tr>
    );
  }
  const spans = (row.line === "removed" ? oldSpans : newSpans)?.[row.at];
  const segments = marks
    ? markSpans(spans ?? [{ role: "plain", text: row.text }], marks)
    : (spans ?? null);
  return (
    <tr className={`diff-line ${row.line}`} data-row={id}>
      <td className="diff-num">{row.old ?? ""}</td>
      <td className="diff-num">{row.new ?? ""}</td>
      <td className="diff-code">
        {segments
          ? segments.map((segment, at) => (
              <span
                key={at}
                className={[
                  // Before its colors arrive, a line keeps its side's color, marks included.
                  spans ? `syntax-${segment.role}` : "",
                  "mark" in segment && segment.mark ? `diff-match ${segment.mark}` : "",
                ]
                  .filter(Boolean)
                  .join(" ")}
              >
                {segment.text}
              </span>
            ))
          : row.text}
      </td>
    </tr>
  );
}

/** Whether a file is on screen or close to it, which is when it's worth highlighting. Once
 * near, it stays so, because its colors are already paid for. */
function useNear(
  target: RefObject<HTMLElement | null>,
  root: RefObject<HTMLElement | null>,
): boolean {
  const [near, setNear] = useState(false);
  useEffect(() => {
    const el = target.current;
    if (!el || near) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) setNear(true);
      },
      { root: root.current, rootMargin: "1000px 0px" },
    );
    observer.observe(el);
    return () => observer.disconnect();
  }, [near]);
  return near;
}

/** Highlights one side of a file off the UI thread, a line of spans per line of text. Each
 * new revision brings new arrays, so it goes by the text, and a file that didn't change keeps
 * its colors. */
function useHighlight(lines: string[] | null, language: string): SyntaxSpan[][] | null {
  const code = lines && lines.length > 0 ? lines.join("\n") : null;
  const [spans, setSpans] = useState<{ code: string; spans: SyntaxSpan[][] } | null>(null);
  useEffect(() => {
    if (code === null || spans?.code === code) return;
    let active = true;
    invoke<SyntaxSpan[]>("highlight_code", { code, language })
      .then((result) => {
        if (active) setSpans({ code, spans: spanLines(result) });
      })
      .catch(() => {});
    return () => {
      active = false;
    };
  }, [code, language]);
  return spans && spans.code === code ? spans.spans : null;
}
