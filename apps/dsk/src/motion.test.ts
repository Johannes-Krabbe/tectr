import assert from "node:assert/strict";
import { test } from "node:test";
import { easedScroll, keepFollowing, nextTyped, typedText } from "./motion.ts";

const FRAME_MS = 1000 / 60;

test("a short way behind, typing is one letter per frame", () => {
  assert.ok(Math.abs(nextTyped(0, 10, FRAME_MS) - 1) < 1e-9);
  assert.ok(Math.abs(nextTyped(4, 10, FRAME_MS) - 5) < 1e-9);
});

test("typing never goes past what has arrived, or backwards", () => {
  assert.equal(nextTyped(9.5, 10, FRAME_MS), 10);
  assert.equal(nextTyped(10, 10, FRAME_MS), 10);
  assert.equal(nextTyped(3, 10, -5), 3);
});

test("a big chunk speeds typing up, so it catches up in about a second or two", () => {
  let shown = 0;
  let frames = 0;
  while (shown < 2000 && frames < 1000) {
    shown = nextTyped(shown, 2000, FRAME_MS);
    frames++;
  }
  assert.ok(frames < 180, `took ${frames} frames`);
});

test("typing stays close behind a stream that keeps coming", () => {
  let shown = 0;
  let length = 0;
  for (let frame = 0; frame < 600; frame++) {
    length += 5; // 300 letters a second
    shown = nextTyped(shown, length, FRAME_MS);
  }
  assert.ok(length - shown < 200, `${length - shown} letters behind`);
});

test("a window that wasn't drawing for a while catches up all at once", () => {
  assert.equal(nextTyped(0, 100, 60_000), 100);
});

test("typed text shows that many letters, and an emoji only whole", () => {
  assert.equal(typedText("hello", 2), "he");
  assert.equal(typedText("hello", 0), "");
  assert.equal(typedText("a\u{1F600}b", 2), "a\u{1F600}");
  assert.equal(typedText("a\u{1F600}b", 3), "a\u{1F600}");
});

test("scrolling moves part of the way each frame and lands exactly", () => {
  const first = easedScroll(0, 100, FRAME_MS);
  assert.ok(first > 0 && first < 100);
  let at = first;
  let frames = 1;
  while (at !== 100 && frames < 60) {
    at = easedScroll(at, 100, FRAME_MS);
    frames++;
  }
  assert.equal(at, 100);
  // A tiny bit animated: a fifth of a second or so, not a glide.
  assert.ok(frames <= 15, `took ${frames} frames`);
});

test("scrolling works in both directions and doesn't move on a frame with no time", () => {
  assert.ok(easedScroll(100, 0, FRAME_MS) < 100);
  assert.equal(easedScroll(40, 100, 0), 40);
});

test("text jumping ahead of the scroll position never stops following", () => {
  // A table appears just as an eased scroll lands: far from the bottom, and not moving up.
  assert.equal(keepFollowing(true, 300, false), true);
});

test("moving up and away stops following, and coming back to the bottom resumes it", () => {
  assert.equal(keepFollowing(true, 300, true), false);
  assert.equal(keepFollowing(false, 10, false), true);
  // A little way up still counts as the bottom.
  assert.equal(keepFollowing(true, 20, true), true);
});

test("reading further up stays that way, even while moving down", () => {
  assert.equal(keepFollowing(false, 300, false), false);
});
