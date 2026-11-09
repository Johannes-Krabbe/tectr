import assert from "node:assert/strict";
import { test } from "node:test";
import {
  chooseHints,
  globalHints,
  hint,
  type HintEntry,
  optionNumbersHint,
  pinnedHints,
} from "./shortcuts.ts";

const nothing = {
  interrupt: false,
  tabs: 0,
  closeTab: false,
  newTab: false,
  topicMcp: false,
  newTopic: false,
  newWorkspace: false,
  topics: 0,
  workspaces: 0,
  sidebar: false,
  settings: false,
};

const pane = hint("Write", "pane.compose");
const composer = hint("Send", "composer.send");
const global = hint("New topic", "global.new_topic");
const pinned = hint("Commands", "global.palette");

const screen: HintEntry[] = [
  { place: "pane", hints: [pane] },
  { place: "typing", hints: [composer] },
  { place: "global", hints: [global] },
  { place: "pinned", hints: [pinned] },
];

test("the pane's keys come before the global ones", () => {
  assert.deepEqual(chooseHints(screen, "pane"), { hints: [pane, global], pinned: [pinned] });
});

test("a text field shows what works while typing instead", () => {
  assert.deepEqual(chooseHints(screen, "typing"), { hints: [composer, global], pinned: [pinned] });
});

test("the sidebar shows its own keys, for the row that has the keyboard", () => {
  const labels = (place: Parameters<typeof chooseHints>[1]) =>
    chooseHints(screen, place).hints.map((shown) => shown.label);
  assert.ok(!labels("sidebar-topic").includes("Write"));
  assert.ok(labels("sidebar-topic").includes("Open"));
  assert.ok(labels("sidebar-topic").includes("Rename"));
  assert.ok(labels("sidebar-topic").includes("Archive"));
  assert.ok(!labels("sidebar-workspace").includes("Rename"));
  assert.ok(labels("sidebar-topic").includes("New topic"));
  assert.ok(!labels("sidebar-workspace").includes("Open"));
  assert.ok(labels("sidebar-device").includes("Device settings"));
  assert.ok(!labels("sidebar-device").includes("Workspace"));
  assert.ok(labels("sidebar").includes("Move"));
});

test("an overlay hides everything behind it, and the latest one wins", () => {
  const sheet = hint("Cancel", "sheet.cancel");
  const picker = hint("Choose", "picker.confirm");
  const withSheet: HintEntry[] = [...screen, { place: "overlay", hints: [sheet] }];
  assert.deepEqual(chooseHints(withSheet, "pane"), { hints: [sheet], pinned: [] });
  const withPicker: HintEntry[] = [...withSheet, { place: "overlay", hints: [picker] }];
  assert.deepEqual(chooseHints(withPicker, "typing"), { hints: [picker], pinned: [] });
});

test("tabs go as far as there are tabs, and aren't shown for just one", () => {
  const tabs = (count: number) =>
    globalHints({ ...nothing, tabs: count }).find((shown) => shown.label === "Tab")?.actions;
  assert.equal(tabs(1), undefined);
  assert.deepEqual(tabs(2), ["global.tab_1", "global.tab_2"]);
  assert.equal(tabs(9)?.length, 9);
  assert.equal(tabs(14)?.length, 9);
});

test("keys that would do nothing right now aren't shown", () => {
  assert.deepEqual(globalHints(nothing), []);
  assert.deepEqual(
    globalHints({ ...nothing, interrupt: true }).map((shown) => shown.actions),
    [["global.interrupt"]],
  );
});

test("moving between topics and workspaces needs somewhere to move to", () => {
  const labels = (topics: number, workspaces: number) =>
    globalHints({ ...nothing, topics, workspaces }).map((shown) => shown.label);
  assert.deepEqual(labels(1, 1), []);
  assert.deepEqual(labels(2, 1), ["Topic"]);
  assert.deepEqual(labels(3, 2), ["Topic", "Workspace"]);
});

test("the number keys stop at the last option", () => {
  assert.deepEqual(optionNumbersHint(2).actions, ["permission.option_1", "permission.option_2"]);
  assert.equal(optionNumbersHint(12).actions.length, 9);
});

test("the next request is only pinned while there is one", () => {
  assert.deepEqual(pinnedHints(0).map((shown) => shown.label), ["Commands"]);
  assert.deepEqual(pinnedHints(2).map((shown) => shown.label), ["Next request", "Commands"]);
});
