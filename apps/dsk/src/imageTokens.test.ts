import assert from "node:assert/strict";
import { test } from "node:test";
import type { PromptImage } from "./engine.ts";
import {
  byNumber,
  imagesIn,
  insertion,
  nextNumber,
  splitTokens,
  tokenAt,
  tokenBeside,
} from "./imageTokens.ts";

const image = (number: number): PromptImage => ({
  id: `id${number}`,
  number,
  mime: "image/png",
  width: 4,
  height: 3,
});

test("the text decides which images go, in the order it names them", () => {
  const known = byNumber([image(1), image(2), image(3)]);
  assert.deepEqual(
    imagesIn("see [Image #3] then [Image #1] and [Image #3] again", known).map((i) => i.number),
    [3, 1],
  );
  // Half deleted, or never pasted, it's only text.
  assert.deepEqual(imagesIn("[Image #2 and [Image #9]", known), []);
});

test("the cursor touches a token from either side or inside it", () => {
  const text = "a [Image #12] b";
  assert.equal(tokenAt(text, 1), undefined);
  assert.equal(tokenAt(text, 2)?.number, 12);
  assert.equal(tokenAt(text, 7)?.number, 12);
  assert.equal(tokenAt(text, 13)?.number, 12);
  assert.equal(tokenAt(text, 14), undefined);
});

test("Backspace and Delete take a whole token only from its edge", () => {
  const text = "a [Image #1] b";
  assert.deepEqual(tokenBeside(text, 12, false), { number: 1, start: 2, end: 12 });
  assert.equal(tokenBeside(text, 11, false), undefined);
  assert.deepEqual(tokenBeside(text, 2, true), { number: 1, start: 2, end: 12 });
  assert.equal(tokenBeside(text, 3, true), undefined);
});

test("numbers go on from what the session sent and what the draft holds", () => {
  assert.equal(nextNumber(0, []), 1);
  assert.equal(nextNumber(4, []), 5);
  assert.equal(nextNumber(4, [image(5), image(7)]), 8);
  assert.equal(nextNumber(9, [image(5)]), 10);
});

test("a pasted token is kept apart from the words around it", () => {
  assert.equal(insertion("", 0, 1), "[Image #1] ");
  assert.equal(insertion("see", 3, 2), " [Image #2] ");
  assert.equal(insertion("see ", 4, 2), "[Image #2] ");
  assert.equal(insertion("a b", 1, 3), " [Image #3]");
  assert.equal(insertion("ab", 1, 3), " [Image #3] ");
});

test("text splits around the tokens of known images only", () => {
  const known = byNumber([image(1)]);
  assert.deepEqual(splitTokens("x [Image #1] y [Image #2]", known), [
    { text: "x " },
    { text: "[Image #1]", image: image(1) },
    { text: " y [Image #2]" },
  ]);
  assert.deepEqual(splitTokens("[Image #1]", known), [{ text: "[Image #1]", image: image(1) }]);
});
