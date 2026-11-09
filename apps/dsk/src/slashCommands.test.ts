import assert from "node:assert/strict";
import { test } from "node:test";
import type { AgentCommand } from "./engine";
import {
  commandQuery,
  completedText,
  matchCommands,
  refusedPrompt,
  sendsAtOnce,
} from "./slashCommands.ts";

const command = (name: string, hint?: string): AgentCommand => ({
  name,
  description: `Runs ${name}`,
  hint,
});

const COMMANDS = [command("review", "pr"), command("usage"), command("mcp"), command("pr-comments")];

const names = (query: string) => matchCommands(COMMANDS, query).map((c) => c.name);

test("the list is only there while a name is being typed", () => {
  assert.equal(commandQuery("/"), "");
  assert.equal(commandQuery("/us"), "us");
  assert.equal(commandQuery("/usage "), null);
  assert.equal(commandQuery("/review 12"), null);
  assert.equal(commandQuery("/us\nmore"), null);
  assert.equal(commandQuery(" /us"), null);
  assert.equal(commandQuery("us"), null);
  assert.equal(commandQuery(""), null);
});

test("names starting with what's typed come before names containing it", () => {
  assert.deepEqual(names(""), ["review", "usage", "mcp", "pr-comments"]);
  assert.deepEqual(names("pr"), ["pr-comments"]);
  assert.deepEqual(names("e"), ["review", "usage", "pr-comments"]);
  assert.deepEqual(names("p"), ["pr-comments", "mcp"]);
  assert.deepEqual(names("U"), ["usage"]);
  assert.deepEqual(names("nothing"), []);
});

test("a command taking input is completed, and one that doesn't is sent", () => {
  assert.equal(completedText(command("review", "pr")), "/review ");
  assert.equal(sendsAtOnce(command("review", "pr")), false);
  assert.equal(sendsAtOnce(command("usage")), true);
});

test("a prompt running a command tectr owns is refused, as the daemon would", () => {
  assert.equal(
    refusedPrompt("  /model opus"),
    "tectr sets the model, so /model can't run here. Change that in tectr instead.",
  );
  assert.notEqual(refusedPrompt("/effort"), null);
  assert.equal(refusedPrompt("/usage"), null);
  assert.equal(refusedPrompt("/models"), null);
  assert.equal(refusedPrompt("what /model am I on"), null);
  assert.equal(refusedPrompt("/toString"), null);
  assert.equal(refusedPrompt("/"), null);
});
