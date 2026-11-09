// Run with: node --test design/logo/logo.test.mjs
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { MARK, staleFiles } from './build.mjs';

test('the mark stays on the 4 unit grid, so small sizes land on whole pixels', () => {
  for (const [x, y] of MARK.flat()) {
    assert.ok(x % 4 === 0 && y % 4 === 0, `${x},${y}`);
    assert.ok(x >= 0 && x <= 48 && y >= 0 && y <= 48, `${x},${y}`);
  }
});

test('logo SVGs and the iOS icon set are current', () => {
  assert.deepEqual(staleFiles(), [], 'run node design/logo/build.mjs');
});
