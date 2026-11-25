import assert from "node:assert/strict";
import { test } from "node:test";
import {
  diffCounts,
  hasDetails,
  inputSummary,
  lineDiff,
  relativePath,
  requestHead,
} from "./toolView.ts";

test("a diff keeps changes with a little around them", () => {
  const before = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"].join("\n");
  const after = ["a", "b", "c", "d", "e", "F", "g", "h", "i", "j"].join("\n");
  assert.deepEqual(lineDiff(before, after), [
    { kind: "gap", count: 2 },
    { kind: "context", text: "c" },
    { kind: "context", text: "d" },
    { kind: "context", text: "e" },
    { kind: "removed", text: "f" },
    { kind: "added", text: "F" },
    { kind: "context", text: "g" },
    { kind: "context", text: "h" },
    { kind: "context", text: "i" },
    { kind: "gap", count: 1 },
  ]);
});

test("a final newline doesn't read as an empty line", () => {
  assert.deepEqual(lineDiff("a\n\n", "a\n\nhello\n"), [
    { kind: "context", text: "a" },
    { kind: "context", text: "" },
    { kind: "added", text: "hello" },
  ]);
  assert.deepEqual(diffCounts(lineDiff("a\nb\n", "a\nc\nd\n")), { added: 2, removed: 1 });
});

test("paths read from the session's folder, or from home outside it", () => {
  assert.equal(relativePath("/Users/me/repo/src/a.rs", "/Users/me/repo"), "src/a.rs");
  assert.equal(relativePath("/Users/me/repo/src/a.rs", "/Users/me/repo/"), "src/a.rs");
  assert.equal(relativePath("/Users/me/other/a.rs", "/Users/me/repo"), "~/other/a.rs");
  assert.equal(relativePath("/etc/hosts", undefined), "/etc/hosts");
});

test("a request says which file it changes and by how much, or what a command is for", () => {
  const edit = {
    title: "Edit src/a.rs",
    input: { file_path: "/r/src/a.rs" },
    content: [{ type: "diff" as const, path: "/r/src/a.rs", old_text: "x\n", new_text: "y\nz\n" }],
  };
  assert.deepEqual(requestHead(edit, "/r"), {
    kind: "file",
    verb: "Edit",
    folder: "src/",
    name: "a.rs",
    added: 2,
    removed: 1,
  });
  const write = { content: [{ type: "diff" as const, path: "/r/b.md", old_text: null, new_text: "hi" }] };
  assert.deepEqual(requestHead(write, "/r"), {
    kind: "file",
    verb: "Write",
    folder: "",
    name: "b.md",
    added: 1,
    removed: 0,
  });
  const described = { title: "`ls`", input: { command: "ls", description: "List files" } };
  assert.deepEqual(requestHead(described, "/r"), { kind: "text", text: "List files" });
  // Titled with the command itself, which the details show anyway.
  assert.equal(requestHead({ title: "`ls`", input: { command: "ls" } }, "/r"), null);
  assert.deepEqual(requestHead({ title: "Fetch a page" }, "/r"), { kind: "text", text: "Fetch a page" });
});

test("a new file is all added", () => {
  assert.deepEqual(lineDiff(null, "x\ny"), [
    { kind: "added", text: "x" },
    { kind: "added", text: "y" },
  ]);
});

test("a tool's input reads as its command, plan or path", () => {
  assert.deepEqual(inputSummary({ input: { command: "ls -la", description: "List" } }), {
    kind: "command",
    text: "ls -la",
  });
  assert.deepEqual(inputSummary({ input: { plan: "# Plan" } }), { kind: "markdown", text: "# Plan" });
  assert.deepEqual(inputSummary({ input: { file_path: "/a/b.rs", old_string: "x" } }), {
    kind: "path",
    text: "/a/b.rs",
  });
  assert.equal(inputSummary({ input: {} }), null);
  assert.equal(inputSummary({}), null);
  assert.equal(inputSummary({ input: { blob: "x".repeat(1000) } }), null);
});

test("a tool with only a title has no details", () => {
  assert.equal(hasDetails({ title: "Thinking" }), false);
  assert.equal(hasDetails({ content: [{ type: "text", text: "  " }] }), false);
  assert.equal(hasDetails({ content: [{ type: "text", text: "out" }] }), true);
});
