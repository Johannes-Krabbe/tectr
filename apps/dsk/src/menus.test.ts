import assert from "node:assert/strict";
import { test } from "node:test";
import { appItems, topicItems, withRuns } from "./menus.ts";

const topic = { name: "Login", toggle: { unread: true, count: 1 }, onNewTab: false };
const ids = (groups: { id: string }[][]) => groups.map((group) => group.map((item) => item.id));

test("a topic's menu groups what's safe, and keeps archiving apart at the end", () => {
  assert.deepEqual(ids(topicItems(topic)), [
    ["rename-topic", "toggle-unread", "move-topic"],
    ["topic-mcp", "import-conversation"],
    ["archive-topic"],
  ]);
});

test("a menu names the action, and the palette also names the topic", () => {
  const [rename, mark] = topicItems(topic)[0];
  assert.equal(rename.label, "Rename");
  assert.equal(rename.inPalette, "Rename Login");
  assert.equal(mark.label, "Mark unread");
  assert.equal(mark.inPalette, "Mark Login unread");
  const read = topicItems({ ...topic, toggle: { unread: false, count: 2 } })[0][1];
  assert.equal(read.label, "Mark read");
});

test("a topic with no tabs has nothing to mark", () => {
  const groups = topicItems({ ...topic, toggle: { unread: false, count: 0 } });
  assert.deepEqual(ids(groups)[0], ["rename-topic", "move-topic"]);
});

test("importing shows the key that works where you are", () => {
  const key = (onNewTab: boolean) => topicItems({ ...topic, onNewTab })[1][1].key;
  assert.equal(key(false), "global.import_conversation");
  assert.equal(key(true), "new_tab.import");
});

test("moving shows the branch the topic is on", () => {
  const move = topicItems({ ...topic, branch: "tectr/login" })[0][2];
  assert.equal(move.detail, "tectr/login");
});

test("the app's menu only offers what would do something", () => {
  const quiet = appItems({ requests: 0, unread: 0, online: false });
  assert.deepEqual(ids(quiet), [[], ["add-device"]]);
  const busy = appItems({ requests: 2, unread: 1, online: true, local: "Laptop" });
  assert.deepEqual(ids(busy), [
    ["next", "mark-all-read", "archived-topics"],
    ["add-device", "pair-machine"],
  ]);
  assert.equal(busy[1][1].label, "Pair another machine");
  assert.equal(busy[1][1].inPalette, "Pair another machine with Laptop");
});

test("every item gets what runs it, and empty groups go", () => {
  const ran: string[] = [];
  const run = (id: string) => () => ran.push(id);
  const groups = withRuns(appItems({ requests: 0, unread: 0, online: false, local: "Laptop" }), {
    next: run("next"),
    "mark-all-read": run("mark-all-read"),
    "archived-topics": run("archived-topics"),
    "add-device": run("add-device"),
    "pair-machine": run("pair-machine"),
  });
  assert.deepEqual(ids(groups), [["add-device", "pair-machine"]]);
  for (const item of groups.flat()) item.run();
  assert.deepEqual(ran, ["add-device", "pair-machine"]);
});
