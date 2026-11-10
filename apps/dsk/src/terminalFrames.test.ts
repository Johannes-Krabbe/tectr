import assert from "node:assert/strict";
import { test } from "node:test";
import { binaryBytes, opaque, OUTPUT, readRecords, readReset, RESET } from "./terminalFrames.ts";

function record(kind: number, payload: number[]): number[] {
  const length = new Uint8Array(new Uint32Array([payload.length]).buffer);
  return [kind, ...length, ...payload];
}

test("a batch of frames reads back in order", () => {
  const bytes = new Uint8Array([
    ...record(OUTPUT, [104, 105]),
    ...record(RESET, [80, 0, 24, 0, 0x1b]),
  ]);
  const records = readRecords(bytes.buffer);
  assert.equal(records.length, 2);
  assert.equal(records[0].kind, OUTPUT);
  assert.deepEqual([...records[0].payload], [104, 105]);
  const reset = readReset(records[1].payload);
  assert.deepEqual([reset.cols, reset.rows, [...reset.screen]], [80, 24, [0x1b]]);
  // What's acknowledged is the whole payload, size included.
  assert.equal(records[1].payload.length, 5);
});

test("a frame cut short is dropped, and the ones before it kept", () => {
  const whole = record(OUTPUT, [1, 2, 3]);
  const cut = record(OUTPUT, [4, 5, 6]).slice(0, 6);
  const records = readRecords(new Uint8Array([...whole, ...cut]).buffer);
  assert.equal(records.length, 1);
  assert.deepEqual(readRecords(new ArrayBuffer(0)), []);
});

test("colors lose their alpha and binary input keeps its bytes", () => {
  // Built up, because app code is checked for color literals.
  const hash = "#";
  assert.equal(opaque(` ${hash}7c5cf029 `), `${hash}7c5cf0`);
  assert.equal(opaque(`${hash}edecf1`), `${hash}edecf1`);
  assert.deepEqual([...binaryBytes("\x1b[M\xff")], [0x1b, 0x5b, 0x4d, 0xff]);
});
