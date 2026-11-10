import assert from "node:assert/strict";
import { test } from "node:test";
import { trackRequests } from "./requests.ts";

type Command = { command: string; topic_id?: string };

/** A device that answers only when told to. */
function device() {
  const sent: {
    machineId: string;
    command: Command;
    answer: (output: string) => void;
    fail: (reason: string) => void;
  }[] = [];
  let time = 0;
  const requests = trackRequests<Command, string>(
    (machineId, command) =>
      new Promise((answer, fail) => sent.push({ machineId, command, answer, fail })),
    () => time,
  );
  return { sent, requests, at: (ms: number) => (time = ms) };
}

const openTerminal: Command = { command: "open_terminal", topic_id: "t1" };

test("a command pressed again before its answer goes out once", async () => {
  const { sent, requests } = device();
  const presses = Array.from({ length: 10 }, () => requests.request("m1", { ...openTerminal }));
  assert.equal(sent.length, 1);
  sent[0].answer("terminal");
  assert.deepEqual(await Promise.all(presses), Array(10).fill("terminal"));
});

test("once answered, the same command goes out again", async () => {
  const { sent, requests } = device();
  const first = requests.request("m1", openTerminal);
  sent[0].answer("one");
  await first;
  const second = requests.request("m1", openTerminal);
  assert.equal(sent.length, 2);
  sent[1].answer("two");
  assert.equal(await second, "two");
});

test("another command, topic or device is its own request", () => {
  const { sent, requests } = device();
  void requests.request("m1", openTerminal);
  void requests.request("m1", { command: "open_terminal", topic_id: "t2" });
  void requests.request("m1", { command: "list_conversations" });
  void requests.request("m2", openTerminal);
  assert.equal(sent.length, 4);
});

test("a failure reaches every press, and the next one tries again", async () => {
  const { sent, requests } = device();
  const presses = [requests.request("m1", openTerminal), requests.request("m1", openTerminal)];
  sent[0].fail("Laptop is offline");
  for (const press of presses) await assert.rejects(press, /offline/);
  void requests.request("m1", openTerminal);
  assert.equal(sent.length, 2);
});

test("a device is waited on since its oldest unanswered command", async () => {
  const { sent, requests, at } = device();
  assert.equal(requests.waitingSince("m1"), null);
  at(100);
  const first = requests.request("m1", openTerminal);
  at(250);
  const second = requests.request("m1", { command: "list_conversations" });
  assert.equal(requests.waitingSince("m1"), 100);
  assert.equal(requests.waitingSince("m2"), null);
  sent[0].answer("terminal");
  await first;
  assert.equal(requests.waitingSince("m1"), 250);
  sent[1].answer("conversations");
  await second;
  assert.equal(requests.waitingSince("m1"), null);
});

test("what's asked in passing isn't waited on", () => {
  const { requests } = device();
  void requests.request("m1", { command: "read_image" });
  void requests.request("m1", { command: "list_folders" });
  assert.equal(requests.waitingSince("m1"), null);
});

test("subscribers hear when a command goes out and when it's answered", async () => {
  const { sent, requests } = device();
  let heard = 0;
  const stop = requests.subscribe(() => heard++);
  const press = requests.request("m1", openTerminal);
  void requests.request("m1", openTerminal);
  assert.equal(heard, 1);
  sent[0].answer("terminal");
  await press;
  assert.equal(heard, 2);
  stop();
  void requests.request("m1", openTerminal);
  assert.equal(heard, 2);
});
