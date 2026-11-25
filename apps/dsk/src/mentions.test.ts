import assert from "node:assert/strict";
import { test } from "node:test";
import { completeMention, mentionAt, shellLine } from "./mentions.ts";

test("a mention is an @ that starts a word, up to the caret", () => {
  assert.deepEqual(mentionAt("see @src/ma", 11), { start: 4, query: "src/ma" });
  assert.deepEqual(mentionAt("@", 1), { start: 0, query: "" });
  assert.deepEqual(mentionAt("(@a", 3), { start: 1, query: "a" });
  assert.equal(mentionAt("mail me@x.com", 13), null);
  assert.equal(mentionAt("see @src done", 13), null);
});

test("completing replaces what's typed and leaves one space", () => {
  const text = "see @src/ma and more";
  const mention = mentionAt(text, 11);
  assert.ok(mention);
  assert.deepEqual(completeMention(text, 11, mention, "src/main.rs"), {
    text: "see @src/main.rs and more",
    caret: 17,
  });
  assert.deepEqual(completeMention("@m", 2, { start: 0, query: "m" }, "main.rs"), {
    text: "@main.rs ",
    caret: 9,
  });
});

test("a bang runs a shell command", () => {
  assert.equal(shellLine("!git status"), "git status");
  assert.equal(shellLine("  ! ls "), "ls");
  assert.equal(shellLine("say !hi"), null);
});
