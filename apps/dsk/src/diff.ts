// What a diff tab shows, worked out from what the daemon sends. Pure, so it's tested.

import type { DiffContent, DiffScope, FileDiff, FileStatus, Hunk } from "./engine";
import type { SyntaxSpan } from "./markdown";

export type LineKind = "context" | "added" | "removed";

/** One row of a file's diff. Numbers are null on the side a line isn't on. `at` is where the
 * line is in the highlighted text of its side: the new side for context and added lines, the
 * old one for removed lines. */
export type DiffRow =
  | { kind: "hunk"; text: string }
  | {
      kind: "line";
      line: LineKind;
      old: number | null;
      new: number | null;
      text: string;
      at: number;
    }
  | { kind: "no_newline" };

/** A file's rows, and the text of each side as highlighting needs it, a line per entry. */
export interface FileRows {
  rows: DiffRow[];
  oldText: string[];
  newText: string[];
}

// Windows line endings would show as a stray character at the end of every line.
const shown = (text: string) => (text.endsWith("\r") ? text.slice(0, -1) : text);

export function fileRows(hunks: Hunk[]): FileRows {
  const rows: DiffRow[] = [];
  const oldText: string[] = [];
  const newText: string[] = [];
  for (const hunk of hunks) {
    const context = hunk.context ? ` ${hunk.context}` : "";
    rows.push({
      kind: "hunk",
      text: `@@ -${hunk.old_start},${hunk.old_lines} +${hunk.new_start},${hunk.new_lines} @@${context}`,
    });
    let old = hunk.old_start;
    let next = hunk.new_start;
    for (const raw of hunk.lines) {
      const sign = raw[0];
      const text = shown(raw.slice(1));
      if (sign === "\\") {
        rows.push({ kind: "no_newline" });
      } else if (sign === "+") {
        rows.push({ kind: "line", line: "added", old: null, new: next++, text, at: newText.length });
        newText.push(text);
      } else if (sign === "-") {
        rows.push({ kind: "line", line: "removed", old: old++, new: null, text, at: oldText.length });
        oldText.push(text);
      } else {
        rows.push({ kind: "line", line: "context", old: old++, new: next++, text, at: newText.length });
        oldText.push(text);
        newText.push(text);
      }
    }
  }
  return { rows, oldText, newText };
}

/** Cuts highlighted text back into its lines. */
export function spanLines(spans: SyntaxSpan[]): SyntaxSpan[][] {
  const lines: SyntaxSpan[][] = [[]];
  for (const span of spans) {
    const parts = span.text.split("\n");
    parts.forEach((part, index) => {
      if (index > 0) lines.push([]);
      if (part) lines[lines.length - 1].push({ role: span.role, text: part });
    });
  }
  return lines;
}

/** What to ask the highlighter for: the extension, or the name of a file that has none, like
 * `Makefile`, which grammars also answer to. */
export function languageOf(path: string): string {
  const name = path.slice(path.lastIndexOf("/") + 1);
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1) : name;
}

export const STATUS_LETTER: Record<FileStatus, string> = {
  added: "A",
  deleted: "D",
  modified: "M",
  renamed: "R",
  copied: "C",
  untracked: "U",
  unknown: "?",
};

export const STATUS_NAME: Record<FileStatus, string> = {
  added: "Added",
  deleted: "Deleted",
  modified: "Modified",
  renamed: "Renamed",
  copied: "Copied",
  untracked: "Untracked",
  unknown: "Changed",
};

/** Tells a file's staged half from the rest of it, which can both be listed. */
export const fileKey = (file: FileDiff) => `${file.staged ? "staged" : "rest"}:${file.path}`;

export interface FileGroup {
  /** Empty when the scope has only one group, which needs no heading. */
  title: string;
  files: FileDiff[];
}

/** The uncommitted scope lists what's staged apart from the rest. */
export function fileGroups(content: DiffContent, scope: DiffScope): FileGroup[] {
  if (scope.kind !== "uncommitted") return [{ title: "", files: content.files }];
  const staged = content.files.filter((file) => file.staged);
  const rest = content.files.filter((file) => !file.staged);
  if (staged.length === 0) return [{ title: "", files: rest }];
  return [
    { title: "Staged", files: staged },
    ...(rest.length > 0 ? [{ title: "Not staged", files: rest }] : []),
  ];
}

export function totals(files: FileDiff[]): { added: number; removed: number } {
  return files.reduce(
    (sum, file) => ({ added: sum.added + file.added, removed: sum.removed + file.removed }),
    { added: 0, removed: 0 },
  );
}

/** A file's folder and name apart, so the name can stay readable when the folder is cut. */
export function splitPath(path: string): { folder: string; name: string } {
  const slash = path.lastIndexOf("/");
  return slash === -1
    ? { folder: "", name: path }
    : { folder: path.slice(0, slash + 1), name: path.slice(slash + 1) };
}

export const otherScope = (scope: DiffScope): DiffScope =>
  scope.kind === "branch" ? { kind: "uncommitted" } : { kind: "branch" };

/** The first of `tops` below `at`, or above it going back. A row within a pixel of where it
 * already is counts as there, so a press always moves. */
export function nextStop(tops: number[], at: number, step: 1 | -1): number | null {
  if (step > 0) return tops.find((top) => top > at + 1) ?? null;
  const above = tops.filter((top) => top < at - 1);
  return above.length > 0 ? above[above.length - 1] : null;
}

/** The file whose section the top of the view is in. */
export function currentIndex(tops: number[], at: number): number {
  let current = 0;
  tops.forEach((top, index) => {
    if (top <= at + 1) current = index;
  });
  return current;
}

/** Where a search found its text: a file, a row of it, and where in that row's text. */
export interface Match {
  file: number;
  row: number;
  start: number;
  end: number;
}

/** Vim's smartcase: a query with a capital letter in it matches case exactly. Matches don't
 * overlap, and come in the order they're shown. */
export function findMatches(files: (DiffRow[] | null)[], query: string): Match[] {
  if (!query) return [];
  const exact = query !== query.toLowerCase();
  const needle = exact ? query : query.toLowerCase();
  const matches: Match[] = [];
  files.forEach((rows, file) => {
    rows?.forEach((row, index) => {
      if (row.kind !== "line") return;
      const text = exact ? row.text : row.text.toLowerCase();
      for (let at = text.indexOf(needle); at !== -1; at = text.indexOf(needle, at + needle.length)) {
        matches.push({ file, row: index, start: at, end: at + needle.length });
      }
    });
  });
  return matches;
}

/** The first match in or after a file, wrapping around to the first. */
export function firstMatchFrom(matches: Match[], file: number): number | null {
  if (matches.length === 0) return null;
  const index = matches.findIndex((match) => match.file >= file);
  return index === -1 ? 0 : index;
}

export interface Segment extends SyntaxSpan {
  mark: "" | "match" | "current";
}

/** Cuts a line's spans where its matches start and end, so each match can be marked. */
export function markSpans(
  spans: SyntaxSpan[],
  marks: { start: number; end: number; current: boolean }[],
): Segment[] {
  const segments: Segment[] = [];
  let at = 0;
  for (const span of spans) {
    let from = 0;
    while (from < span.text.length) {
      const pos = at + from;
      const inside = marks.find((m) => m.start <= pos && pos < m.end);
      const next = inside
        ? inside.end
        : Math.min(...marks.filter((m) => m.start > pos).map((m) => m.start), Infinity);
      const to = Math.min(span.text.length, next - at);
      segments.push({
        role: span.role,
        text: span.text.slice(from, to),
        mark: inside ? (inside.current ? "current" : "match") : "",
      });
      from = to;
    }
    at += span.text.length;
  }
  return segments;
}

/** Stands for what a file's diff shows, so marking it viewed lasts until it changes. */
export function fingerprint(file: FileDiff): string {
  const text =
    file.body.kind === "text"
      ? file.body.hunks.map((hunk) => hunk.lines.join("\n")).join("\n@@\n")
      : `${file.body.kind}:${file.added}:${file.removed}`;
  // FNV-1a, which is plenty to tell one version of a file from the next.
  let hash = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    hash ^= text.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193);
  }
  return `${text.length}:${(hash >>> 0).toString(36)}`;
}

/** Reads the viewed files a tab kept, file key to fingerprint. Anything else is dropped,
 * because local storage can hold what an older version wrote. */
export function parseViewed(text: string | null): Record<string, string> {
  if (!text) return {};
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return {};
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return {};
  return Object.fromEntries(
    Object.entries(parsed).filter((entry): entry is [string, string] => typeof entry[1] === "string"),
  );
}
