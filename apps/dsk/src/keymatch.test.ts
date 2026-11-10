import assert from "node:assert/strict";
import { test } from "node:test";
import {
  chordMatches,
  chordOf,
  formatChord,
  formatKeys,
  formatSequence,
  isBareChord,
  type KeyPress,
  sequenceState,
  usesCommand,
} from "./keymatch.ts";

const press = (key: string, extra: Partial<KeyPress> = {}): KeyPress => ({
  key,
  code: "",
  metaKey: false,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  ...extra,
});

test("mod means cmd on macOS and ctrl elsewhere", () => {
  assert.ok(chordMatches("mod+k", press("k", { metaKey: true }), true));
  assert.ok(!chordMatches("mod+k", press("k", { ctrlKey: true }), true));
  assert.ok(chordMatches("mod+k", press("k", { ctrlKey: true }), false));
  assert.ok(!chordMatches("mod+k", press("k", { metaKey: true }), false));
  assert.ok(chordMatches("ctrl+n", press("n", { ctrlKey: true }), true));
});

test("extra modifiers don't match", () => {
  assert.ok(!chordMatches("mod+t", press("t", { metaKey: true, shiftKey: true }), true));
  assert.ok(!chordMatches("mod+t", press("t", { metaKey: true, altKey: true }), true));
  assert.ok(!chordMatches("k", press("k", { metaKey: true }), true));
  assert.ok(!chordMatches("mod+k", press("k"), true));
});

test("shift has to match for letters and named keys", () => {
  assert.ok(chordMatches("mod+shift+n", press("N", { metaKey: true, shiftKey: true }), true));
  assert.ok(!chordMatches("mod+shift+n", press("n", { metaKey: true }), true));
  assert.ok(chordMatches("shift+enter", press("Enter", { shiftKey: true }), true));
  assert.ok(!chordMatches("enter", press("Enter", { shiftKey: true }), true));
  assert.ok(chordMatches("shift+r", press("R", { shiftKey: true }), true));
});

test("shift is ignored for a symbol that needs it on some layouts", () => {
  assert.ok(chordMatches("/", press("/"), true));
  assert.ok(chordMatches("/", press("/", { shiftKey: true, code: "Digit7" }), true));
  assert.ok(chordMatches("?", press("?", { shiftKey: true }), true));
  assert.ok(chordMatches("mod+[", press("[", { metaKey: true }), true));
});

test("named keys match the browser's names", () => {
  assert.ok(chordMatches("up", press("ArrowUp"), true));
  assert.ok(chordMatches("alt+shift+down", press("ArrowDown", { altKey: true, shiftKey: true }), true));
  assert.ok(chordMatches("escape", press("Escape"), true));
  assert.ok(chordMatches("space", press(" "), true));
  assert.ok(chordMatches("mod+plus", press("+", { metaKey: true }), true));
});

test("option and shift match the key under them", () => {
  assert.ok(chordMatches("alt+k", press("˚", { altKey: true, code: "KeyK" }), true));
  assert.ok(chordMatches("alt+1", press("¡", { altKey: true, code: "Digit1" }), true));
  assert.ok(chordMatches("mod+shift+1", press("!", { metaKey: true, shiftKey: true, code: "Digit1" }), true));
  assert.ok(!chordMatches("mod+1", press("!", { metaKey: true, shiftKey: true, code: "Digit1" }), true));
});

test("recording produces what matching expects", () => {
  const cases: [KeyPress, string][] = [
    [press("k", { metaKey: true }), "mod+k"],
    [press("N", { metaKey: true, shiftKey: true }), "mod+shift+n"],
    [press("ArrowUp", { altKey: true }), "alt+up"],
    [press("˚", { altKey: true, code: "KeyK" }), "alt+k"],
    [press("?", { shiftKey: true }), "?"],
    [press(" "), "space"],
    [press("Escape"), "escape"],
    [press("n", { ctrlKey: true }), "ctrl+n"],
    [press("n", { ctrlKey: true, metaKey: true }), "mod+ctrl+n"],
  ];
  for (const [event, chord] of cases) {
    assert.equal(chordOf(event, true), chord);
    assert.ok(chordMatches(chord, event, true), chord);
  }
  assert.equal(chordOf(press("n", { ctrlKey: true }), false), "mod+n");
  assert.equal(chordOf(press("n", { metaKey: true }), false), "cmd+n");
});

test("a modifier on its own isn't a shortcut", () => {
  for (const key of ["Meta", "Control", "Alt", "Shift", "CapsLock"]) {
    assert.equal(chordOf(press(key), true), null);
  }
});

test("only keys with cmd, ctrl or alt, and function keys, are safe while typing", () => {
  for (const chord of ["y", "1", "shift+y", "enter", "up", "/"]) assert.ok(isBareChord(chord), chord);
  for (const chord of ["mod+y", "cmd+y", "ctrl+y", "alt+y", "f5"]) assert.ok(!isBareChord(chord), chord);
});

test("a sequence waits for its next key, then completes", () => {
  assert.equal(sequenceState("g g", [press("g")], true), "prefix");
  assert.equal(sequenceState("g g", [press("g"), press("g")], true), "full");
  assert.equal(sequenceState("z z", [press("g")], true), null);
  assert.equal(sequenceState("a b c", [press("a"), press("b")], true), "prefix");
  assert.equal(sequenceState("a b c", [press("a"), press("b"), press("c")], true), "full");
});

test("a key that doesn't continue a sequence starts over", () => {
  assert.equal(sequenceState("g g", [press("g"), press("j")], true), null);
  // Like Vim, what came before doesn't stop it from starting again.
  assert.equal(sequenceState("g g", [press("x"), press("g")], true), "prefix");
  assert.equal(sequenceState("g g", [press("x"), press("g"), press("g")], true), "full");
  assert.equal(sequenceState("a b c", [press("a"), press("b"), press("a")], true), "prefix");
  assert.equal(sequenceState("a b c", [press("a"), press("x"), press("c")], true), null);
});

test("sequences can have modifiers and shift", () => {
  const shiftT = press("T", { shiftKey: true });
  assert.equal(sequenceState("g shift+t", [press("g"), shiftT], true), "full");
  assert.equal(sequenceState("g t", [press("g"), shiftT], true), null);
  const ctrlW = press("w", { ctrlKey: true });
  assert.equal(sequenceState("ctrl+w w", [ctrlW], true), "prefix");
  assert.equal(sequenceState("ctrl+w w", [ctrlW, press("w")], true), "full");
});

test("sequences read the way Vim writes them", () => {
  assert.equal(formatSequence("g g", true), "gg");
  assert.equal(formatSequence("z z", true), "zz");
  assert.equal(formatSequence("g shift+t", true), "gT");
  assert.equal(formatSequence("ctrl+w w", true), "⌃W w");
  assert.equal(formatSequence("shift+g", true), "⇧G");
  assert.equal(formatSequence("mod+k", true), "⌘K");
});

test("labels read the way each platform writes them", () => {
  assert.equal(formatChord("mod+shift+n", true), "⌘⇧N");
  assert.equal(formatChord("alt+shift+up", true), "⌥⇧↑");
  assert.equal(formatChord("mod+.", true), "⌘.");
  assert.equal(formatChord("escape", true), "Esc");
  assert.equal(formatChord("enter", true), "↵");
  assert.equal(formatChord("mod+shift+n", false), "Ctrl+Shift+N");
  assert.equal(formatChord("mod+plus", true), "⌘+");
});

const keys = (...list: string[]) => list.map((key) => ({ key, hold_ms: 0, repeat: false }));

test("a pair of keys reads as one", () => {
  assert.equal(formatKeys(keys("k", "j"), true), "K/J");
  assert.equal(formatKeys(keys("up", "down"), true), "↑/↓");
  assert.equal(formatKeys(keys("g g", "shift+g"), true), "gg/⇧G");
});

test("a pair only says its shared modifiers once", () => {
  assert.equal(formatKeys(keys("alt+k", "alt+j"), true), "⌥K/J");
  assert.equal(formatKeys(keys("alt+k", "alt+j"), false), "Alt+K/J");
  assert.equal(formatKeys(keys("mod+[", "mod+]"), true), "⌘[/]");
  assert.equal(formatKeys(keys("mod+k", "alt+j"), true), "⌘K/⌥J");
});

test("numbers in a row read as a range", () => {
  const tabs = [1, 2, 3, 4, 5, 6, 7, 8, 9].map((n) => `mod+${n}`);
  assert.equal(formatKeys(keys(...tabs), true), "⌘1-9");
  assert.equal(formatKeys(keys(...tabs), false), "Ctrl+1-9");
  assert.equal(formatKeys(keys("1", "2", "3"), true), "1-3");
  assert.equal(formatKeys(keys("g 1", "g 2"), true), "g1-2");
  assert.equal(formatKeys(keys("mod+1"), true), "⌘1");
});

test("a range doesn't cover keys it doesn't have", () => {
  assert.equal(formatKeys(keys("mod+1", "mod+2", "mod+4", "mod+5"), true), "⌘1-2/4-5");
  assert.equal(formatKeys(keys("mod+1", "alt+2"), true), "⌘1/⌥2");
  assert.equal(formatKeys(keys("mod+2", "mod+1"), true), "⌘2/1");
});

test("holding is said once, and only where it applies", () => {
  const held = { key: "mod+w", hold_ms: 500, repeat: false };
  assert.equal(formatKeys([held], true), "Hold ⌘W");
  assert.equal(formatKeys([held, { ...held, key: "mod+q" }], true), "Hold ⌘W/Q");
  assert.equal(formatKeys([held, ...keys("mod+q")], true), "Hold ⌘W/⌘Q");
  assert.equal(
    formatKeys([{ ...held, key: "mod+1" }, { ...held, key: "mod+2" }], true),
    "Hold ⌘1-2",
  );
});

test("only keys that start with Cmd reach the app from a terminal", () => {
  assert.equal(usesCommand("mod+t"), true);
  assert.equal(usesCommand("cmd+shift+n"), true);
  assert.equal(usesCommand("mod+w q"), true);
  assert.equal(usesCommand("ctrl+c"), false);
  assert.equal(usesCommand("escape"), false);
  assert.equal(usesCommand("g t"), false);
  assert.equal(usesCommand("ctrl+w mod+q"), false);
});
