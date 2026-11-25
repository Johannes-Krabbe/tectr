// What a tool call's details show: what it was given and what it gave back, kept short.

import type { ToolCallPatch } from "./engine";

export type DiffLine =
  | { kind: "context" | "added" | "removed"; text: string }
  /** Unchanged lines left out, far from any change. */
  | { kind: "gap"; count: number };

/** Past this many lines on a side, an edit shows its new text instead of a diff. Diffing is
 * quadratic, and nobody reads a diff that long before allowing it anyway. */
const MAX_DIFF_LINES = 400;
/** How many unchanged lines stay around each change. */
const CONTEXT = 3;

function linesOf(text: string | null): string[] {
  if (!text) return [];
  const lines = text.split("\n");
  // A final newline ends the last line rather than starting an empty one.
  if (lines[lines.length - 1] === "") lines.pop();
  return lines;
}

/** The lines that changed between two texts, with a little of what didn't around them. Lines
 * far from any change are left out, each run of them as one gap. */
export function lineDiff(oldText: string | null, newText: string): DiffLine[] {
  const before = linesOf(oldText);
  const after = linesOf(newText);
  if (before.length > MAX_DIFF_LINES || after.length > MAX_DIFF_LINES) {
    return after.map((text) => ({ kind: "added", text }));
  }
  // The longest common subsequence, from the end, so walking forward picks the first match.
  const common: number[][] = Array.from({ length: before.length + 1 }, () =>
    new Array<number>(after.length + 1).fill(0),
  );
  for (let i = before.length - 1; i >= 0; i--) {
    for (let j = after.length - 1; j >= 0; j--) {
      common[i][j] =
        before[i] === after[j]
          ? common[i + 1][j + 1] + 1
          : Math.max(common[i + 1][j], common[i][j + 1]);
    }
  }
  const all: { kind: "context" | "added" | "removed"; text: string }[] = [];
  let i = 0;
  let j = 0;
  while (i < before.length || j < after.length) {
    if (i < before.length && j < after.length && before[i] === after[j]) {
      all.push({ kind: "context", text: before[i] });
      i++;
      j++;
    } else if (j < after.length && (i === before.length || common[i][j + 1] > common[i + 1][j])) {
      all.push({ kind: "added", text: after[j] });
      j++;
    } else {
      all.push({ kind: "removed", text: before[i] });
      i++;
    }
  }
  const near = all.map((_, index) =>
    all
      .slice(Math.max(0, index - CONTEXT), index + CONTEXT + 1)
      .some((other) => other.kind !== "context"),
  );
  const kept: DiffLine[] = [];
  all.forEach((line, index) => {
    if (near[index]) {
      kept.push(line);
      return;
    }
    const last = kept[kept.length - 1];
    if (last?.kind === "gap") last.count++;
    else kept.push({ kind: "gap", count: 1 });
  });
  return kept;
}

/** How many lines a diff adds and removes. */
export function diffCounts(lines: DiffLine[]): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const line of lines) {
    if (line.kind === "added") added++;
    else if (line.kind === "removed") removed++;
  }
  return { added, removed };
}

/** A path as it reads from the session's folder, or from home when it's outside it. */
export function relativePath(path: string, folder: string | undefined): string {
  const root = folder?.replace(/\/+$/, "");
  if (root && path.startsWith(`${root}/`)) return path.slice(root.length + 1);
  return path.replace(/^\/(Users|home)\/[^/]+(?=\/|$)/, "~");
}

/** What a request says it would do, on its one line above the details: the file an edit
 * changes, with how much, or what a command is for. Null when the details say it all. */
export type RequestHead =
  | { kind: "file"; verb: string; folder: string; name: string; added: number; removed: number }
  | { kind: "text"; text: string };

export function requestHead(tool: ToolCallPatch, folder: string | undefined): RequestHead | null {
  const diffs = (tool.content ?? []).flatMap((item) => (item.type === "diff" ? [item] : []));
  if (diffs.length === 1) {
    const diff = diffs[0];
    const shown = relativePath(diff.path, folder);
    const slash = shown.lastIndexOf("/");
    return {
      kind: "file",
      verb: diff.old_text === null ? "Write" : "Edit",
      folder: shown.slice(0, slash + 1),
      name: shown.slice(slash + 1),
      ...diffCounts(lineDiff(diff.old_text, diff.new_text)),
    };
  }
  const input = inputSummary(tool);
  const title = tool.title?.trim() || null;
  if (input?.kind === "command") {
    const description = stringField(tool.input, "description");
    if (description) return { kind: "text", text: description };
    // Some agents title a command with the command itself, which the details already show.
    const bare = title?.replace(/^`+|`+$/g, "").trim();
    return title && bare !== input.text.trim() ? { kind: "text", text: title } : null;
  }
  return title ? { kind: "text", text: title } : null;
}

function stringField(input: unknown, key: string): string | null {
  if (input === null || typeof input !== "object") return null;
  const value: unknown = Reflect.get(input, key);
  return typeof value === "string" && value.trim() !== "" ? value : null;
}

/** The most of a tool's text output a step shows before it offers the rest. */
export const SHOWN_OUTPUT_CHARS = 2000;

/** The part of a tool's input worth reading, which is how agents describe it: a command, a
 * path or a plan. Anything else shows as JSON, unless it's long. */
export function inputSummary(
  tool: ToolCallPatch,
): { kind: "command" | "path" | "markdown" | "json"; text: string } | null {
  const input = tool.input;
  if (input === null || typeof input !== "object") return null;
  const field = (key: string) => stringField(input, key);
  const command = field("command");
  if (command) return { kind: "command", text: command };
  const plan = field("plan");
  if (plan) return { kind: "markdown", text: plan };
  const path = field("file_path") ?? field("path") ?? field("notebook_path");
  if (path) return { kind: "path", text: path };
  const json = JSON.stringify(input, null, 2);
  return json.length <= 600 && json !== "{}" ? { kind: "json", text: json } : null;
}

/** Whether a tool call has anything to show beyond its title. */
export function hasDetails(tool: ToolCallPatch): boolean {
  return (
    inputSummary(tool) !== null ||
    (tool.content ?? []).some(
      (content) =>
        (content.type === "text" && content.text.trim() !== "") || content.type === "diff",
    )
  );
}
