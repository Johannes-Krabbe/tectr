import assert from "node:assert/strict";
import { test } from "node:test";
import {
  chooseTab,
  closeNewTab,
  goTo,
  landingAfterArchiving,
  landingIn,
  type Navigation,
  NEW_TAB,
  openNewTab,
  parseNavigation,
  shownTab,
  topicTabs,
  type View,
} from "./navigation.ts";

const empty = parseNavigation(null);
const topic: View = { kind: "topic", machineId: "m", topicId: "t" };
const other: View = { kind: "topic", machineId: "m", topicId: "t2" };
const newTopic: View = { kind: "new_topic", machineId: "m", workspaceId: "w" };
const device: View = { kind: "device_settings", machineId: "m" };
const settings = (section: string): View => ({ kind: "app_settings", section });

test("a new client opens on the empty state", () => {
  assert.equal(empty.view, null);
});

test("a restarted client opens where it was", () => {
  const navigation = chooseTab(goTo(empty, topic, "w"), "t", "s1");
  assert.deepEqual(parseNavigation(JSON.stringify(navigation)), navigation);
});

test("something unreadable counts as a new client", () => {
  for (const stored of ["{not json", "null", "3", '"text"']) {
    assert.deepEqual(parseNavigation(stored), empty);
  }
});

test("going to a workspace lands on the topic last open in it", () => {
  const workspace = { id: "w", topics: [{ id: "t" }, { id: "t2" }, { id: "t3" }] };
  assert.deepEqual(landingIn(goTo(empty, topic, "w"), "m", workspace), topic);
});

test("a workspace not visited here lands on its newest topic", () => {
  const workspace = { id: "w", topics: [{ id: "t0" }, { id: "t" }] };
  assert.deepEqual(landingIn(empty, "m", workspace), topic);
  assert.deepEqual(landingIn(goTo(empty, other, "w"), "m", workspace), topic);
});

test("a workspace with no topics lands on the new topic screen", () => {
  assert.deepEqual(landingIn(empty, "m", { id: "w", topics: [] }), newTopic);
});

test("a remembered view missing newer fields gets their defaults", () => {
  const stored = JSON.stringify({ view: topic });
  assert.deepEqual(parseNavigation(stored), { ...empty, view: topic });
});

test("leaving settings goes back to what was open before them", () => {
  let navigation: Navigation = goTo(empty, topic, "w");
  navigation = goTo(navigation, settings("general"));
  navigation = goTo(navigation, settings("keybinds"));
  navigation = goTo(navigation, device);
  assert.deepEqual(navigation.beforeSettings, topic);
});

test("leaving the new topic screen goes back to what was open before it", () => {
  let navigation: Navigation = goTo(empty, topic, "w");
  navigation = goTo(navigation, newTopic);
  navigation = goTo(navigation, { ...newTopic, workspaceId: "w2" });
  assert.deepEqual(navigation.beforeNewTopic, topic);
});

test("a trip to settings doesn't change where the new topic screen goes back to", () => {
  let navigation: Navigation = goTo(empty, topic, "w");
  navigation = goTo(navigation, newTopic);
  navigation = goTo(navigation, settings("general"));
  navigation = goTo(navigation, newTopic);
  assert.deepEqual(navigation.beforeNewTopic, topic);
});

test("the new topic screen has nothing to go back to when no topic was open", () => {
  let navigation: Navigation = goTo(empty, settings("general"));
  navigation = goTo(navigation, newTopic);
  assert.equal(navigation.beforeNewTopic, null);
  navigation = goTo(goTo(empty, device), newTopic);
  assert.equal(navigation.beforeNewTopic, null);
});

test("settings reopens at the last section", () => {
  const navigation = goTo(goTo(empty, settings("keybinds")), topic, "w");
  assert.equal(navigation.settingsSection, "keybinds");
});

test("each workspace remembers its last topic", () => {
  let navigation = goTo(empty, topic, "w");
  navigation = goTo(navigation, other, "w");
  navigation = goTo(navigation, { kind: "topic", machineId: "m", topicId: "t3" }, "w2");
  assert.deepEqual(navigation.lastTopics, { w: "t2", w2: "t3" });
});

test("the chosen tab shows while it exists, even with newer ones", () => {
  assert.equal(shownTab(["a", "b", "c"], "a", ["a", "b"]), "a");
});

test("without a choice, the newest tab shows", () => {
  assert.equal(shownTab(["a", "b"], undefined, []), "b");
  assert.equal(shownTab([], undefined, []), undefined);
});

test("a closed tab gives way to the one after it, or else the one before", () => {
  assert.equal(shownTab(["a", "c"], "b", ["a", "b", "c"]), "c");
  assert.equal(shownTab(["a", "b"], "c", ["a", "b", "c"]), "b");
  assert.equal(shownTab(["a"], "b", ["a", "b", "c"]), "a");
  assert.equal(shownTab([], "a", ["a"]), undefined);
});

test("a tab closed while nothing was watching falls back to the newest", () => {
  assert.equal(shownTab(["a", "b"], "gone", []), "b");
});

test("archiving the shown topic lands next to it, and on a new topic when it was the last", () => {
  const workspace = { id: "w", topics: [{ id: "a" }, { id: "b" }, { id: "c" }] };
  const landing = (topicId: string, topics = workspace.topics) =>
    landingAfterArchiving("m", { id: "w", topics }, topicId);
  assert.deepEqual(landing("b"), { kind: "topic", machineId: "m", topicId: "c" });
  assert.deepEqual(landing("c"), { kind: "topic", machineId: "m", topicId: "b" });
  assert.deepEqual(landing("a", [{ id: "a" }]), newTopic);
});

test("a topic with no sessions always has a new tab", () => {
  assert.deepEqual(topicTabs(empty, "t", []), [NEW_TAB]);
  assert.deepEqual(topicTabs(closeNewTab(openNewTab(empty, "t"), "t"), "t", []), [NEW_TAB]);
});

test("a new tab opens after the sessions and is shown", () => {
  const navigation = openNewTab(empty, "t");
  const tabs = topicTabs(navigation, "t", ["a", "b"]);
  assert.deepEqual(tabs, ["a", "b", NEW_TAB]);
  assert.equal(shownTab(tabs, navigation.tabs.t, ["a", "b"]), NEW_TAB);
  assert.deepEqual(topicTabs(navigation, "other", ["c"]), ["c"]);
});

test("opening the new tab again goes back to the same one", () => {
  const navigation = openNewTab(chooseTab(openNewTab(empty, "t"), "t", "a"), "t");
  assert.deepEqual(navigation.newTabs, ["t"]);
  assert.equal(navigation.tabs.t, NEW_TAB);
});

test("closing the new tab shows the tab before it", () => {
  const opened = openNewTab(empty, "t");
  const closed = closeNewTab(opened, "t");
  const tabs = topicTabs(closed, "t", ["a", "b"]);
  assert.deepEqual(tabs, ["a", "b"]);
  assert.equal(shownTab(tabs, closed.tabs.t, ["a", "b", NEW_TAB]), "b");
});

test("an agent started from the new tab takes its place", () => {
  const started = chooseTab(closeNewTab(openNewTab(empty, "t"), "t"), "t", "c");
  const tabs = topicTabs(started, "t", ["a", "c"]);
  assert.deepEqual(tabs, ["a", "c"]);
  assert.equal(shownTab(tabs, started.tabs.t, ["a", NEW_TAB]), "c");
});
