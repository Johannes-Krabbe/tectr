import assert from "node:assert/strict";
import { test } from "node:test";
import type { Folder } from "./engine";
import {
  afterPaste,
  highlightWhileTyping,
  isLoopback,
  moveHighlight,
  splitPath,
  STARTING_PATH,
  suggest,
} from "./pathInput.ts";

const folder = (name: string, is_repo = false): Folder => ({
  name,
  path: `/home/me/${name}`,
  is_repo,
});

const HOME = [
  folder(".config"),
  folder("Desktop"),
  folder("Developer", true),
  folder("Documents"),
  folder("Downloads"),
  folder("music"),
];

const typedFor = (partial: string) =>
  suggest("~/", partial, HOME, []).map((suggestion) => suggestion.typed);

test("what's typed splits into the folder to ask about and the start of a name", () => {
  assert.deepEqual(splitPath("~/"), { parent: "~/", partial: "" });
  assert.deepEqual(splitPath("~/D"), { parent: "~/", partial: "D" });
  assert.deepEqual(splitPath("~/Developer/"), { parent: "~/Developer/", partial: "" });
  assert.deepEqual(splitPath("~/Developer/te"), { parent: "~/Developer/", partial: "te" });
  assert.deepEqual(splitPath("/Us"), { parent: "/", partial: "Us" });
  assert.deepEqual(splitPath("  ~/D "), { parent: "~/", partial: "D" });
});

test("nothing is asked until it's a path that can be listed", () => {
  for (const typed of ["", "~", "D", "./x", "~root/x"]) assert.equal(splitPath(typed), null);
});

test("suggestions match the start of the name, ignoring case", () => {
  assert.deepEqual(typedFor("D"), ["~/Desktop/", "~/Developer/", "~/Documents/", "~/Downloads/"]);
  assert.deepEqual(typedFor("do"), ["~/Documents/", "~/Downloads/"]);
  assert.deepEqual(typedFor("M"), ["~/music/"]);
  assert.deepEqual(typedFor("zzz"), []);
  assert.deepEqual(typedFor("own"), []);
});

test("folders starting with a dot wait for a dot", () => {
  assert.ok(!typedFor("").includes("~/.config/"));
  assert.deepEqual(typedFor("."), ["~/.config/"]);
});

test("a repo is tagged, and one that's already a workspace says so instead", () => {
  const tags = (workspaces: string[]) =>
    suggest("~/", "Dev", HOME, workspaces).map((suggestion) => suggestion.tag);
  assert.deepEqual(tags([]), ["git"]);
  assert.deepEqual(tags(["/home/me/Developer"]), ["added"]);
  assert.deepEqual(
    suggest("~/", "Doc", HOME, []).map((suggestion) => suggestion.tag),
    [null],
  );
});

test("suggestions keep the path as it was typed, and the absolute one beside it", () => {
  const [row] = suggest("~/", "Dev", HOME, []);
  assert.equal(row.typed, "~/Developer/");
  assert.equal(row.path, "/home/me/Developer");
});

test("typing part of a name highlights the first match, and a finished folder nothing", () => {
  assert.equal(highlightWhileTyping("D"), 0);
  assert.equal(highlightWhileTyping(""), null);
  assert.equal(highlightWhileTyping(undefined), null);
});

test("moving passes through having nothing highlighted", () => {
  assert.equal(moveHighlight(null, 1, 3), 0);
  assert.equal(moveHighlight(0, 1, 3), 1);
  assert.equal(moveHighlight(2, 1, 3), null);
  assert.equal(moveHighlight(null, -1, 3), 2);
  assert.equal(moveHighlight(1, -1, 3), 0);
  assert.equal(moveHighlight(0, -1, 3), null);
  assert.equal(moveHighlight(null, 1, 0), null);
  assert.equal(moveHighlight(0, -1, 0), null);
});

test("a pasted path replaces the starting path, and nothing else", () => {
  assert.equal(afterPaste(STARTING_PATH, "/srv/repo"), "/srv/repo");
  assert.equal(afterPaste(STARTING_PATH, "~/Developer/tectr\n"), "~/Developer/tectr");
  assert.equal(afterPaste(STARTING_PATH, "~"), "~");
  assert.equal(afterPaste(STARTING_PATH, "Developer"), null);
  assert.equal(afterPaste("~/Dev", "/srv/repo"), null);
});

test("only an address on this machine counts as local", () => {
  assert.ok(isLoopback("ws://127.0.0.1:7767"));
  assert.ok(isLoopback("ws://localhost:7767"));
  assert.ok(isLoopback("ws://[::1]:7767"));
  assert.ok(!isLoopback("ws://100.64.0.3:7767"));
  assert.ok(!isLoopback("ws://macbook.tailnet.ts.net:7767"));
  assert.ok(!isLoopback("ws://127.0.0.1.example.com:7767"));
  assert.ok(!isLoopback("not a url"));
});
