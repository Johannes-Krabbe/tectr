import assert from "node:assert/strict";
import { test } from "node:test";
import { countdown, qrPath, spacedDigits } from "./pairing.ts";

test("draws each run of dark modules as one rectangle", () => {
  assert.equal(
    qrPath([
      [true, true, false, true],
      [false, false, false, false],
      [false, true, true, true],
    ]),
    "M0 0h2v1h-2zM3 0h1v1h-1zM1 2h3v1h-3z",
  );
});

test("draws nothing for an empty code", () => {
  assert.equal(qrPath([[false, false]]), "");
});

test("counts down in whole seconds, rounding up", () => {
  assert.equal(countdown(300_000), "5:00");
  assert.equal(countdown(299_001), "5:00");
  assert.equal(countdown(299_000), "4:59");
  assert.equal(countdown(9_500), "0:10");
  assert.equal(countdown(1), "0:01");
  assert.equal(countdown(0), "0:00");
  assert.equal(countdown(-5_000), "0:00");
});

test("splits pairing digits in the middle", () => {
  assert.equal(spacedDigits("482913"), "482 913");
});
