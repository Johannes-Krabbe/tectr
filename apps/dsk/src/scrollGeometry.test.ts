import assert from "node:assert/strict";
import { test } from "node:test";
import { scrollThumb } from "./scrollGeometry.ts";

const area = { left: 100, top: 50, width: 400, height: 204 };

test("the thumb shows how much is visible and how far it's scrolled", () => {
  const top = scrollThumb("y", area, { client: 200, size: 800, offset: 0 });
  assert.deepEqual(top, { left: 492, top: 52, width: 6, height: 50, scrollPerPixel: 4 });
  const bottom = scrollThumb("y", area, { client: 200, size: 800, offset: 600 });
  assert.equal(bottom.top + bottom.height, 50 + 204 - 2);
});

test("a long page keeps a thumb big enough to grab, and overscroll stays on the track", () => {
  const thumb = scrollThumb("y", area, { client: 200, size: 100_000, offset: 1e9 });
  assert.equal(thumb.height, 24);
  assert.equal(thumb.top + thumb.height, 50 + 204 - 2);
});

test("sideways the thumb runs along the bottom", () => {
  const thumb = scrollThumb("x", area, { client: 396, size: 792, offset: 396 });
  assert.equal(thumb.top, 50 + 204 - 2 - 6);
  assert.equal(thumb.left + thumb.width, 100 + 400 - 2);
});
