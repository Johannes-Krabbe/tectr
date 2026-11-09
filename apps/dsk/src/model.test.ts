import assert from "node:assert/strict";
import { test } from "node:test";
import type { ArchiveRisk, DaemonState, Provider, ProviderModel, Session, Topic } from "./engine";
import {
  archiveLosses,
  archiveSummary,
  effortForModel,
  keepsAwake,
  moveChoices,
  sessionState,
  topicState,
  topicWorktree,
  unreadToggle,
  withKeepAwakeToggled,
} from "./model.ts";

const smart: ProviderModel = {
  id: "smart",
  name: "Smart",
  efforts: [
    { id: "low", name: "Low" },
    { id: "high", name: "High" },
  ],
};
const cheap: ProviderModel = { id: "cheap", name: "Cheap", efforts: [] };
const provider = {
  agent: "fake",
  name: "Fake",
  modes: ["auto"],
  models: [smart, cheap],
  default_model: "smart",
  default_effort: "low",
} as Provider;

test("switching model keeps the effort if the new model has it", () => {
  assert.equal(effortForModel(provider, smart, "high"), "high");
});

test("an effort the new model lacks becomes its default", () => {
  assert.equal(effortForModel(provider, smart, "extreme"), "low");
  assert.equal(effortForModel(provider, smart, null), "low");
});

test("a model without efforts has no effort", () => {
  assert.equal(effortForModel(provider, cheap, "high"), null);
});

const repo = { id: "repo", path: "/repo", branch: "main" };
const first = { id: "first", path: "/data/first", branch: "tectr/first" };
const second = { id: "second", path: "/data/second", branch: "part-2" };
const inFolder = (worktree_id: string, worktrees: Topic["worktrees"]): Topic => ({
  id: "t",
  name: "Topic",
  worktree_id,
  worktrees,
  sessions: [],
});

test("a topic works in the folder it points at, not its first", () => {
  assert.equal(topicWorktree(inFolder("second", [first, second])), second);
  assert.equal(topicWorktree(inFolder("gone", [first])), undefined);
});

test("a topic can move to a new worktree, the repo, or a worktree it left", () => {
  const targets = (topic: Topic) =>
    moveChoices(topic, "/repo").map((choice) => [choice.label, choice.target]);
  assert.deepEqual(targets(inFolder("repo", [repo])), [
    ["A new worktree", { kind: "new_worktree" }],
  ]);
  assert.deepEqual(targets(inFolder("second", [repo, first, second])), [
    ["A new worktree", { kind: "new_worktree" }],
    ["The repo", { kind: "root" }],
    ["tectr/first", { kind: "worktree", worktree_id: "first" }],
  ]);
  assert.equal(moveChoices(inFolder("second", [second]), "/repo")[0].detail, "From part-2");
});

test("archiving says what it stops and exactly what it loses", () => {
  const nothing: ArchiveRisk = { running: 1, worktrees: [] };
  assert.deepEqual(archiveLosses(nothing), []);
  assert.equal(archiveSummary(nothing), "Stops 1 running agent. Its tabs come back when it's restored.");
  const risk: ArchiveRisk = {
    running: 0,
    worktrees: [
      { path: "/data/first", branch: "tectr/first", uncommitted: 3, unpushed: 1 },
      { path: "/data/second", branch: "tectr/second", uncommitted: 0, unpushed: 2 },
    ],
  };
  assert.deepEqual(archiveLosses(risk), [
    "3 uncommitted files in /data/first",
    "1 commit on tectr/first that no other branch or remote has",
    "2 commits on tectr/second that no other branch or remote has",
  ]);
  assert.equal(archiveSummary(risk), "Its worktree has work that would be lost.");
});

const tab = (id: string, status: Session["status"], unread = false): Session => ({
  id,
  agent: "claude",
  mode: "auto",
  model: null,
  status,
  title: null,
  unread,
});
const topicWith = (...sessions: Session[]): Topic => ({
  id: "t",
  name: "Topic",
  worktree_id: "wt",
  worktrees: [],
  sessions,
});

test("a finished tab is green until it's read, even after its agent stops", () => {
  assert.equal(sessionState(tab("s", "done", true), true), "done");
  assert.equal(sessionState(tab("s", "done"), true), "read");
  assert.equal(sessionState(tab("s", "stopped", true), true), "done");
  assert.equal(sessionState(tab("s", "stopped"), true), "idle");
  assert.equal(sessionState(tab("s", "done", true), false), "stale");
});

test("something to read outranks an agent still working, but not one asking", () => {
  const working = tab("a", "working");
  const unread = tab("b", "done", true);
  assert.equal(topicState(topicWith(working, unread), true), "done");
  assert.equal(topicState(topicWith(working, tab("b", "done")), true), "working");
  assert.equal(topicState(topicWith(unread, tab("c", "waiting_for_you")), true), "waiting");
});

test("toggling reads every unread tab, or else marks the finished ones unread", () => {
  const read = unreadToggle(topicWith(tab("a", "done", true), tab("b", "done"), tab("c", "working")));
  assert.equal(read.unread, false);
  assert.deepEqual(read.sessions.map((s) => s.id), ["a"]);
  const unread = unreadToggle(topicWith(tab("a", "done"), tab("b", "ready"), tab("c", "done")));
  assert.equal(unread.unread, true);
  assert.deepEqual(unread.sessions.map((s) => s.id), ["a", "c"]);
});

test("keeping awake is on until turned off, and toggling it keeps the other settings", () => {
  const state = (settings?: DaemonState["settings"]): DaemonState => ({
    machine: { id: "m", name: "m" },
    workspaces: [],
    approvals: [],
    settings,
  });
  assert.equal(keepsAwake(state()), true);
  assert.deepEqual(withKeepAwakeToggled(state()), { keep_awake: false });
  assert.equal(keepsAwake(state({ keep_awake: false })), false);
  assert.deepEqual(withKeepAwakeToggled(state({ keep_awake: false })), { keep_awake: true });
});
