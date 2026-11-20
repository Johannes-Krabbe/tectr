import assert from "node:assert/strict";
import { test } from "node:test";
import { appItems, topicItems, withRuns, workspaceItems } from "./menus.ts";

const topic = { toggle: { unread: true, count: 1 } };
const ids = (groups: { id: string }[][]) => groups.map((group) => group.map((item) => item.id));

test("a topic's menu groups what's safe, and keeps archiving apart at the end", () => {
  assert.deepEqual(ids(topicItems(topic)), [
    ["rename-topic", "toggle-unread", "move-topic"],
    ["open-workspace", "import-conversation"],
    ["archive-topic"],
  ]);
});

test("a menu names the action, and the key where there is one", () => {
  const [rename, mark] = topicItems(topic)[0];
  assert.equal(rename.label, "Rename");
  assert.equal(rename.key, "sidebar.rename");
  assert.equal(mark.label, "Mark unread");
  const read = topicItems({ toggle: { unread: false, count: 2 } })[0][1];
  assert.equal(read.label, "Mark read");
});

test("a topic with no tabs has nothing to mark", () => {
  const groups = topicItems({ toggle: { unread: false, count: 0 } });
  assert.deepEqual(ids(groups)[0], ["rename-topic", "move-topic"]);
});

test("what's done rarely has no key, only its place in the menu", () => {
  const [open, importing] = topicItems(topic)[1];
  assert.equal(open.label, "Open its workspace");
  assert.equal(open.key, undefined);
  assert.equal(importing.key, undefined);
});

test("a workspace's menu keeps closing apart, on the key that archives a topic", () => {
  const groups = workspaceItems();
  assert.deepEqual(ids(groups), [["new-topic"], ["close-workspace"]]);
  assert.equal(groups[1][0].label, "Close workspace");
  assert.equal(groups[1][0].key, "sidebar.archive");
});

test("the app's menu only offers what would do something", () => {
  assert.deepEqual(ids(appItems({ requests: 0, unread: 0 })), [[]]);
  assert.deepEqual(ids(appItems({ requests: 2, unread: 1 })), [["next", "mark-all-read"]]);
});

test("every item gets what runs it, and empty groups go", () => {
  const ran: string[] = [];
  const run = (id: string) => () => ran.push(id);
  const runs = { next: run("next"), "mark-all-read": run("mark-all-read") };
  assert.deepEqual(withRuns(appItems({ requests: 0, unread: 0 }), runs), []);
  const groups = withRuns(appItems({ requests: 0, unread: 3 }), runs);
  assert.deepEqual(ids(groups), [["mark-all-read"]]);
  for (const item of groups.flat()) item.run();
  assert.deepEqual(ran, ["mark-all-read"]);
});
