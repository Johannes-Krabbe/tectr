import assert from "node:assert/strict";
import { test } from "node:test";
import { initialState, type Register, selectionOf, type VimMode, type VimState, vimKey } from "./vim.ts";

const EMPTY: Register = { text: "", linewise: false };

/** Types keys into text with the cursor at |, the way the text box would: an edit is made,
 * and a key vim leaves alone types itself in insert mode. Returns the text with the cursor. */
function type(
  start: string,
  keys: string,
  { mode = "normal", register = EMPTY, now = () => 0 }: { mode?: VimMode; register?: Register; now?: () => number } = {},
) {
  let text = start.replace("|", "");
  let cursor = start.indexOf("|");
  let state: VimState = initialState(mode);
  let yanked = register;
  const passed: string[] = [];
  for (const key of keys.match(/<(?:[A-Za-z]+|C-.)>|./gsu) ?? []) {
    const result = vimKey(state, key, { text, cursor, register: yanked }, { escape: "jk", timeoutMs: 500 }, now());
    state = result.state;
    if (!result.handled) {
      passed.push(key);
      if (state.mode === "insert" && !key.startsWith("<")) {
        text = text.slice(0, cursor) + key + text.slice(cursor);
        cursor += key.length;
      }
      continue;
    }
    if (result.yank) yanked = result.yank;
    if (result.edit) text = text.slice(0, result.edit.from) + result.edit.insert + text.slice(result.edit.to);
    cursor = result.cursor;
  }
  return { text: `${text.slice(0, cursor)}|${text.slice(cursor)}`, mode: state.mode, register: yanked, passed, state };
}

const after = (start: string, keys: string, options?: Parameters<typeof type>[2]) => type(start, keys, options).text;

test("h and l stay on the line, and l stops on its last character", () => {
  assert.equal(after("ab|cd\nef", "l"), "abc|d\nef");
  assert.equal(after("ab|cd\nef", "5l"), "abc|d\nef");
  assert.equal(after("ab|cd\nef", "5h"), "|abcd\nef");
});

test("j and k keep the column they started from across a short line", () => {
  assert.equal(after("abc|def\nx\nabcdef", "j"), "abcdef\n|x\nabcdef");
  assert.equal(after("abc|def\nx\nabcdef", "jj"), "abcdef\nx\nabc|def");
  assert.equal(after("abcdef\nx\nabc|def", "2k"), "abc|def\nx\nabcdef");
  assert.equal(after("ab|c\nd", "9j"), "abc\n|d");
});

test("w, b and e move by words and punctuation, and W, B and E only by blanks", () => {
  assert.equal(after("|foo.bar baz", "w"), "foo|.bar baz");
  assert.equal(after("|foo.bar baz", "W"), "foo.bar |baz");
  assert.equal(after("|foo bar", "e"), "fo|o bar");
  assert.equal(after("foo bar|", "b"), "foo |bar");
  assert.equal(after("foo.bar |baz", "B"), "|foo.bar baz");
  assert.equal(after("|a b c d", "3w"), "a b c |d");
});

test("an empty line is a stop for w and b", () => {
  assert.equal(after("|foo\n\nbar", "w"), "foo\n|\nbar");
  assert.equal(after("foo\n\n|bar", "b"), "foo\n|\nbar");
});

test("0, ^, $, gg and G", () => {
  assert.equal(after("  fo|o", "0"), "|  foo");
  assert.equal(after("  fo|o", "^"), "  |foo");
  assert.equal(after("|foo\nbar", "$"), "fo|o\nbar");
  assert.equal(after("|foo\nbar", "2$"), "foo\nba|r");
  assert.equal(after("a\nb\n  |c", "gg"), "|a\nb\n  c");
  assert.equal(after("|a\nb\n  c", "G"), "a\nb\n  |c");
  assert.equal(after("|a\nb\nc", "2G"), "a\n|b\nc");
});

test("f, t, F and T find on the line, and ; and , repeat it", () => {
  assert.equal(after("|a,b,c", "f,"), "a|,b,c");
  assert.equal(after("|a,b,c", "f,;"), "a,b|,c");
  assert.equal(after("|a,b,c", "t,"), "|a,b,c");
  assert.equal(after("|xa,b,c", "t,"), "x|a,b,c");
  assert.equal(after("|xa,b,c", "t,;"), "xa,|b,c");
  assert.equal(after("a,b,|c", "F,"), "a,b|,c");
  assert.equal(after("a,bc|d", "T,"), "a,|bcd");
  assert.equal(after("a,b,c|", "F,,"), "a,b|,c");
  assert.equal(after("|abc", "fz"), "|abc");
});

test("dw and cw stop at the end of the line, and cw keeps the space after the word", () => {
  assert.equal(after("foo |bar\nbaz", "dw"), "foo| \nbaz");
  assert.equal(after("|foo bar", "dw"), "|bar");
  assert.equal(after("|foo bar", "cwx"), "x| bar");
  assert.equal(after("|foo bar baz", "c2wx"), "x| baz");
  assert.equal(after("|foo bar baz", "d2w"), "|baz");
});

test("de, d$, D, C and dt delete what the motion covers", () => {
  assert.equal(after("|foo bar", "de"), "| bar");
  assert.equal(after("f|oo bar\nx", "d$"), "|f\nx");
  assert.equal(after("f|oo bar\nx", "D"), "|f\nx");
  assert.equal(after("f|oo bar", "Cy"), "fy|");
  assert.equal(after("|foo(bar)", "dt("), "|(bar)");
  assert.equal(after("|foo(bar)", "df("), "|bar)");
});

test("dd deletes lines and puts the cursor on the line that's left", () => {
  assert.equal(after("a\n|b\n  c", "dd"), "a\n  |c");
  assert.equal(after("a\nb\n|c", "dd"), "a\n|b");
  assert.equal(after("|a\nb\nc", "2dd"), "|c");
  assert.equal(after("|a", "dd"), "|");
  assert.equal(after("|a\nb\nc", "dj"), "|c");
  assert.equal(after("a\nb\n|c", "dk"), "|a");
  assert.equal(after("a\n|b\nc", "dG"), "|a");
});

test("cc and S empty the line and start typing on it", () => {
  const result = type("a\n|bcd\ne", "ccx");
  assert.equal(result.text, "a\nx|\ne");
  assert.equal(result.mode, "insert");
  assert.equal(after("|bcd", "Sy"), "y|");
});

test("x, X and s work on characters, with a count", () => {
  assert.equal(after("a|bcd", "x"), "a|cd");
  assert.equal(after("a|bcd", "9x"), "|a");
  assert.equal(after("ab|cd", "X"), "a|cd");
  assert.equal(after("a|bcd", "2sz"), "az|d");
  assert.equal(after("a|😀b", "x"), "a|b");
});

test("yanking and putting characters and whole lines", () => {
  assert.equal(after("|foo bar", "ywP"), "foo| foo bar");
  assert.equal(after("|foo bar", "ywp"), "ffoo| oo bar");
  assert.equal(after("|a\nb", "yyp"), "a\n|a\nb");
  assert.equal(after("a\n|b", "yyP"), "a\n|b\nb");
  assert.equal(after("|a\nb", "yy2p"), "a\n|a\na\nb");
  assert.equal(after("|a\nb", "ddp"), "b\n|a");
  assert.deepEqual(type("|foo bar", "yiw").register, { text: "foo", linewise: false });
  assert.deepEqual(type("|foo\nbar", "yj").register, { text: "foo\nbar\n", linewise: true });
});

test("a register from the clipboard puts the way it was yanked", () => {
  assert.equal(after("|ab", "p", { register: { text: "X", linewise: false } }), "a|Xb");
  assert.equal(after("|ab\ncd", "p", { register: { text: "line\n", linewise: true } }), "ab\n|line\ncd");
});

test("text objects for words, quotes and brackets", () => {
  assert.equal(after("foo b|ar baz", "diw"), "foo | baz");
  assert.equal(after("foo b|ar baz", "daw"), "foo |baz");
  assert.equal(after("foo b|ar", "daw"), "fo|o");
  assert.equal(after('say "he|llo" now', 'ci"x'), 'say "x|" now');
  assert.equal(after('say "he|llo" now', 'da"'), "say |now");
  assert.equal(after('|say "hello"', 'di"'), 'say "|"');
  assert.equal(after("f(a, (b|), c)", "di("), "f(a, (|), c)");
  assert.equal(after("f(a, (b), |c)", "dib"), "f(|)");
  assert.equal(after("f(a|)", "da("), "|f");
  assert.equal(after("{\n  fo|o\n}", "di{"), "{\n|}");
  assert.equal(after("x <a|b> y", "di<"), "x <|> y");
});

test("visual mode selects from where it started to the cursor, inclusive", () => {
  assert.equal(after("|abcd", "vld"), "|cd");
  assert.equal(after("ab|cd", "vhd"), "a|d");
  assert.equal(after("a\n|b\nc\nd", "Vjd"), "a\n|d");
  assert.deepEqual(type("|abc def", "vey").register, { text: "abc", linewise: false });
  assert.equal(after("foo b|ar baz", "viwd"), "foo | baz");
  assert.equal(after("|abcd", "vlcX"), "X|cd");
  const swapped = type("a|bcd", "vllo");
  assert.equal(swapped.text, "a|bcd");
  assert.equal(swapped.state.anchor, 3);
});

test("putting over a selection replaces it and yanks what was there", () => {
  const result = type("|foo bar", "vep", { register: { text: "X", linewise: false } });
  assert.equal(result.text, "|X bar");
  assert.deepEqual(result.register, { text: "foo", linewise: false });
  assert.equal(after("|a\nb", "Vp", { register: { text: "z\n", linewise: true } }), "|z\nb");
});

test("v and V switch between each other and leave, and Esc leaves", () => {
  assert.equal(type("|ab", "vV").mode, "visual_line");
  assert.equal(type("|ab", "vv").mode, "normal");
  assert.equal(type("|ab", "v<Esc>").mode, "normal");
});

test("i, a, I, A, o and O start typing in the right place", () => {
  assert.equal(after("a|bc", "iX"), "aX|bc");
  assert.equal(after("a|bc", "aX"), "abX|c");
  assert.equal(after("  a|bc", "IX"), "  X|abc");
  assert.equal(after("a|bc", "AX"), "abcX|");
  assert.equal(after("a|bc\nd", "oX"), "abc\nX|\nd");
  assert.equal(after("a\n|bc", "OX"), "a\nX|\nbc");
});

test("Esc leaves insert mode onto the character just typed", () => {
  const result = type("ab|", "<Esc>", { mode: "insert" });
  assert.equal(result.text, "a|b");
  assert.equal(result.mode, "normal");
  assert.equal(after("|ab", "<Esc>", { mode: "insert" }), "|ab");
  assert.equal(after("a|b", "<C-[>", { mode: "insert" }), "|ab");
});

test("the two letter escape takes back the first letter, if the second comes soon enough", () => {
  const quick = type("a|", "jk", { mode: "insert" });
  assert.equal(quick.text, "|a");
  assert.equal(quick.mode, "normal");
  let time = 0;
  const slow = type("a|", "jk", { mode: "insert", now: () => (time += 600) });
  assert.equal(slow.text, "ajk|");
  assert.equal(slow.mode, "insert");
  assert.equal(type("a|", "xk", { mode: "insert" }).text, "axk|");
});

test("r replaces, J joins and ~ switches case", () => {
  assert.equal(after("|abc", "rx"), "|xbc");
  assert.equal(after("|abc", "2rx"), "x|xc");
  assert.equal(after("|abc", "5rx"), "|abc");
  assert.equal(after("|a\n  b\nc", "J"), "a| b\nc");
  assert.equal(after("|a\n  b\nc", "3J"), "a b| c");
  assert.equal(after("|a\n)", "J"), "a|)");
  assert.equal(after("|aBc", "2~"), "Ab|c");
  assert.equal(after("|ab cd", "vl~"), "|AB cd");
});

test("u and Ctrl+R ask the text box to undo and redo", () => {
  const undo = vimKey(initialState("normal"), "u", { text: "", cursor: 0, register: EMPTY }, { escape: "", timeoutMs: 0 }, 0);
  assert.deepEqual(undo.history, { action: "undo", count: 1 });
  const redo = type("|a", "3<C-r>");
  assert.equal(redo.mode, "normal");
});

test("keys vim doesn't use are swallowed in normal mode, but not Esc or Ctrl keys", () => {
  const result = type("|ab", "Zq");
  assert.equal(result.text, "|ab");
  assert.deepEqual(result.passed, []);
  assert.deepEqual(type("|ab", "<Esc>").passed, ["<Esc>"]);
  assert.deepEqual(type("|ab", "<C-x>").passed, ["<C-x>"]);
  // Esc cancels a command that's started, rather than leaving.
  const cancelled = type("|ab", "d<Esc>x");
  assert.deepEqual(cancelled.passed, []);
  assert.equal(cancelled.text, "|b");
});

test("counts multiply, and 0 is a count only after another digit", () => {
  assert.equal(after("|a b c d e f g", "2d2w"), "|e f g");
  assert.equal(after("|abcdefghijkl", "10l"), "abcdefghij|kl");
  assert.equal(after("abc|d", "0"), "|abcd");
});

test("normal mode shows a block on the character, and visual mode the whole selection", () => {
  const normal = initialState("normal");
  assert.deepEqual(selectionOf(normal, "abc", 1), { start: 1, end: 2, backward: false });
  assert.deepEqual(selectionOf(normal, "a\n\nb", 2), { start: 2, end: 2, backward: false });
  assert.deepEqual(selectionOf(normal, "😀b", 0), { start: 0, end: 2, backward: false });
  const visual: VimState = { ...normal, mode: "visual", anchor: 3 };
  assert.deepEqual(selectionOf(visual, "abcdef", 1), { start: 1, end: 4, backward: true });
  const lines: VimState = { ...normal, mode: "visual_line", anchor: 0 };
  assert.deepEqual(selectionOf(lines, "ab\ncd\nef", 4), { start: 0, end: 5, backward: false });
});
