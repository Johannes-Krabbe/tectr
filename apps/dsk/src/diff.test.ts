import assert from "node:assert/strict";
import { test } from "node:test";
import type { DiffRow } from "./diff.ts";
import {
  currentIndex,
  fileGroups,
  fileRows,
  findMatches,
  fingerprint,
  firstMatchFrom,
  hunkAt,
  languageOf,
  markSpans,
  nextStop,
  parseViewed,
  pathsOf,
  spanLines,
  splitPath,
  totals,
} from "./diff.ts";
import type { DiffContent, FileDiff } from "./engine";

const file = (path: string, staged: boolean, added = 1, removed = 0): FileDiff => ({
  path,
  status: "modified",
  staged,
  added,
  removed,
  body: { kind: "text", hunks: [] },
});

test("rows number each side and point removed lines at the old text", () => {
  const { rows, oldText, newText } = fileRows([
    {
      old_start: 3,
      old_lines: 3,
      new_start: 3,
      new_lines: 3,
      context: "fn   main()\t",
      lines: [" keep\r", "-old", "+new", " end", "\\ No newline at end of file"],
    },
  ]);
  assert.deepEqual(rows, [
    { kind: "hunk", range: "@@ -3,3 +3,3 @@", context: "fn main()", hunk: 0 },
    { kind: "line", line: "context", old: 3, new: 3, text: "keep", at: 0 },
    { kind: "line", line: "removed", old: 4, new: null, text: "old", at: 1 },
    { kind: "line", line: "added", old: null, new: 4, text: "new", at: 1 },
    { kind: "line", line: "context", old: 5, new: 5, text: "end", at: 2 },
    { kind: "no_newline" },
  ]);
  assert.deepEqual(oldText, ["keep", "old", "end"]);
  assert.deepEqual(newText, ["keep", "new", "end"]);
});

test("highlighted text goes back into its lines, empty ones included", () => {
  const lines = spanLines([
    { role: "keyword", text: "let" },
    { role: "plain", text: " a\n\nb" },
    { role: "string", text: "\"x\"\n" },
  ]);
  assert.deepEqual(lines, [
    [
      { role: "keyword", text: "let" },
      { role: "plain", text: " a" },
    ],
    [],
    [
      { role: "plain", text: "b" },
      { role: "string", text: "\"x\"" },
    ],
    [],
  ]);
});

test("the language is the extension, or the name of a file without one", () => {
  assert.equal(languageOf("src/main.rs"), "rs");
  assert.equal(languageOf("Makefile"), "Makefile");
  assert.equal(languageOf("a.b/.gitignore"), ".gitignore");
});

test("only uncommitted changes with something staged get headings", () => {
  const content: DiffContent = {
    revision: 1,
    files: [file("b", true), file("a", false), file("b", false)],
  };
  assert.deepEqual(
    fileGroups(content, { kind: "uncommitted" }).map((g) => [g.title, g.files.length]),
    [
      ["Staged", 1],
      ["Not staged", 2],
    ],
  );
  assert.deepEqual(
    fileGroups({ revision: 1, files: [file("a", false)] }, { kind: "uncommitted" }).map(
      (g) => g.title,
    ),
    [""],
  );
  assert.deepEqual(
    fileGroups(content, { kind: "branch" }).map((g) => g.title),
    [""],
  );
  assert.deepEqual(totals(content.files), { added: 3, removed: 0 });
});

test("paths split into folder and name", () => {
  assert.deepEqual(splitPath("a/b/c.ts"), { folder: "a/b/", name: "c.ts" });
  assert.deepEqual(splitPath("c.ts"), { folder: "", name: "c.ts" });
});

test("moving between files always moves, and knows which file is on top", () => {
  const tops = [0, 100, 300];
  assert.equal(nextStop(tops, 0, 1), 100);
  assert.equal(nextStop(tops, 100.5, 1), 300);
  assert.equal(nextStop(tops, 300, 1), null);
  assert.equal(nextStop(tops, 150, -1), 100);
  assert.equal(nextStop(tops, 100, -1), 0);
  assert.equal(nextStop(tops, 0, -1), null);
  assert.equal(currentIndex(tops, 0), 0);
  assert.equal(currentIndex(tops, 250), 1);
  assert.equal(currentIndex(tops, 1000), 2);
});

const rows = (...texts: string[]): DiffRow[] =>
  texts.map((text, at) => ({ kind: "line", line: "context", old: at, new: at, text, at }));

test("search is smartcase, finds every match in a line, and skips files without lines", () => {
  const files = [rows("Foo foo", "bar"), null, rows("xfoo")];
  assert.deepEqual(
    findMatches(files, "foo").map((m) => [m.file, m.row, m.start]),
    [
      [0, 0, 0],
      [0, 0, 4],
      [2, 0, 1],
    ],
  );
  assert.deepEqual(
    findMatches(files, "Foo").map((m) => [m.file, m.row, m.start]),
    [[0, 0, 0]],
  );
  assert.deepEqual(findMatches(files, ""), []);
  const matches = findMatches(files, "foo");
  assert.equal(firstMatchFrom(matches, 1), 2);
  assert.equal(firstMatchFrom(matches, 5), 0);
  assert.equal(firstMatchFrom([], 0), null);
});

test("matches cut across highlighted spans", () => {
  const segments = markSpans(
    [
      { role: "keyword", text: "let" },
      { role: "plain", text: " foo = 1" },
    ],
    [
      { start: 2, end: 5, current: false },
      { start: 6, end: 7, current: true },
    ],
  );
  assert.deepEqual(
    segments.map((s) => [s.text, s.role, s.mark]),
    [
      ["le", "keyword", ""],
      ["t", "keyword", "match"],
      [" f", "plain", "match"],
      ["o", "plain", ""],
      ["o", "plain", "current"],
      [" = 1", "plain", ""],
    ],
  );
});

test("a file's fingerprint changes with its lines", () => {
  const base = file("a", false);
  const edited: FileDiff = {
    ...base,
    body: { kind: "text", hunks: [{ old_start: 1, old_lines: 1, new_start: 1, new_lines: 1, lines: ["+x"] }] },
  };
  assert.equal(fingerprint(base), fingerprint({ ...base }));
  assert.notEqual(fingerprint(base), fingerprint(edited));
});

test("viewed files read back from storage, and junk reads as none", () => {
  assert.deepEqual(parseViewed('{"rest:a":"1:x","bad":3}'), { "rest:a": "1:x" });
  assert.deepEqual(parseViewed("[1]"), {});
  assert.deepEqual(parseViewed("{"), {});
  assert.deepEqual(parseViewed(null), {});
});

test("the hunk the keys act on stays in the file at the top", () => {
  const hunks = [
    { file: 0, hunk: 0, top: 0 },
    { file: 0, hunk: 1, top: 200 },
    { file: 1, hunk: 0, top: 500 },
  ];
  assert.equal(hunkAt(hunks, 0, 0), 0);
  assert.equal(hunkAt(hunks, 0, 300), 1);
  // The next file's header is at the top, but its first hunk hasn't reached it yet.
  assert.equal(hunkAt(hunks, 1, 480), 0);
  assert.equal(hunkAt(hunks, 2, 900), null);
});

test("a renamed file stages both its paths", () => {
  assert.deepEqual(pathsOf({ ...file("new", true), old_path: "old" }), ["new", "old"]);
  assert.deepEqual(pathsOf(file("a", false)), ["a"]);
});
