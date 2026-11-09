import assert from "node:assert/strict";
import { test } from "node:test";
import type { Provider, ProviderModel, Session, Topic } from "./engine";
import { deleteTopicEffect, effortForModel, sessionWorktree } from "./model.ts";

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

test("a session's worktree is the one it points at, not the topic's first", () => {
  const session: Session = {
    id: "s",
    worktree_id: "second",
    agent: "claude",
    mode: "auto",
    model: null,
    status: "done",
    title: null,
  };
  const first = { id: "first", path: "/data/first", branch: "tectr/first" };
  const second = { id: "second", path: "/data/second", branch: "tectr/second" };
  const topic = (worktrees: Topic["worktrees"]): Topic => ({
    id: "t",
    name: "Topic",
    worktrees,
    sessions: [session],
  });
  assert.equal(sessionWorktree(topic([first, second]), session), second);
  assert.equal(sessionWorktree(topic([first]), session), undefined);
});

test("deleting says what it does to the agents and the worktree", () => {
  const session: Session = {
    id: "s",
    worktree_id: "wt",
    agent: "claude",
    mode: "auto",
    model: null,
    status: "done",
    title: null,
  };
  const topic = (path: string, sessions: Session[]): Topic => ({
    id: "t",
    name: "Topic",
    worktrees: [{ id: "wt", path, branch: "b" }],
    sessions,
  });
  assert.equal(
    deleteTopicEffect(topic("/data/wt", []), "/repo"),
    "Removes its worktree, but keeps uncommitted work and unmerged commits.",
  );
  assert.equal(
    deleteTopicEffect(topic("/repo", [session]), "/repo"),
    "Stops its agents. The repo stays as it is.",
  );
});
