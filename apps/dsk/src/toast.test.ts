import assert from "node:assert/strict";
import { test } from "node:test";
import { shownToasts, showToast } from "./toast.ts";

const texts = () => shownToasts().map((toast) => toast.text);

test("a toast is up for a few seconds, then gone", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  showToast("Added 3 connectors");
  assert.deepEqual(texts(), ["Added 3 connectors"]);
  t.mock.timers.tick(2999);
  assert.deepEqual(texts(), ["Added 3 connectors"]);
  t.mock.timers.tick(1);
  assert.deepEqual(texts(), []);
});

test("saying the same thing again keeps one line and starts its time over", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  showToast("No new connectors");
  t.mock.timers.tick(2000);
  showToast("No new connectors");
  assert.deepEqual(texts(), ["No new connectors"]);
  t.mock.timers.tick(2000);
  assert.deepEqual(texts(), ["No new connectors"]);
  t.mock.timers.tick(1000);
  assert.deepEqual(texts(), []);
});

test("each goes when its own time is up, and only a few are up at once", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  showToast("one");
  t.mock.timers.tick(1000);
  showToast("two");
  t.mock.timers.tick(2000);
  assert.deepEqual(texts(), ["two"]);
  for (const text of ["three", "four", "five"]) showToast(text);
  assert.deepEqual(texts(), ["three", "four", "five"]);
  t.mock.timers.tick(3000);
  assert.deepEqual(texts(), []);
});

test("the settings can keep one up for longer", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  showToast("slow", 10);
  t.mock.timers.tick(9999);
  assert.deepEqual(texts(), ["slow"]);
  t.mock.timers.tick(1);
  assert.deepEqual(texts(), []);
});
