// A small Vim for text boxes. It's pure, so it's tested on its own and can sit under any
// editor: a key and the text go in, and out comes the edit, where the cursor goes and what
// was yanked. VimBox.tsx puts it under a textarea. Only what a message box needs is here,
// so there are no macros, marks, named registers, ex commands or dot repeat.

export type VimMode = "insert" | "normal" | "visual" | "visual_line";

/** What yanking and deleting keep, and putting puts back. */
export interface Register {
  text: string;
  /** Whole lines, which put goes in between lines rather than into one. */
  linewise: boolean;
}

export interface VimOptions {
  /** Two letters that leave insert mode when typed one right after the other, or "". */
  escape: string;
  /** How soon the second letter has to follow, like Vim's timeoutlen. */
  timeoutMs: number;
}

type FindKind = "f" | "t" | "F" | "T";

export interface VimState {
  mode: VimMode;
  /** The keys of a command that isn't finished yet, like 2d or f. */
  pending: string[];
  /** Where the selection started, in visual mode. */
  anchor: number;
  /** The column j and k aim for, so a short line on the way doesn't change it. */
  column: number | null;
  lastFind: { kind: FindKind; char: string } | null;
  /** The last letter typed in insert mode and when, for the two letter escape. */
  typed: { key: string; at: number } | null;
}

export const initialState = (mode: VimMode): VimState => ({
  mode,
  pending: [],
  anchor: 0,
  column: null,
  lastFind: null,
  typed: null,
});

export interface Edit {
  from: number;
  to: number;
  insert: string;
}

export interface VimInput {
  text: string;
  cursor: number;
  register: Register;
}

export interface VimResult {
  state: VimState;
  /** False leaves the key to the text box: typing in insert mode, or Esc with nothing to cancel. */
  handled: boolean;
  /** Where the cursor is once `edit` is made. */
  cursor: number;
  edit?: Edit;
  yank?: Register;
  /** Undo and redo belong to the text box, so its own history has everything in it. */
  history?: { action: "undo" | "redo"; count: number };
}

// ----- Lines and characters -----

export const lineStart = (text: string, pos: number) =>
  pos <= 0 ? 0 : text.lastIndexOf("\n", pos - 1) + 1;

export function lineEnd(text: string, pos: number): number {
  const end = text.indexOf("\n", pos);
  return end < 0 ? text.length : end;
}

const isBlank = (ch: string | undefined) => ch === " " || ch === "\t";

function firstNonBlank(text: string, pos: number): number {
  const end = lineEnd(text, pos);
  let at = lineStart(text, pos);
  while (at < end && isBlank(text[at])) at++;
  return at;
}

const graphemes = new Intl.Segmenter(undefined, { granularity: "grapheme" });

/** Where the character at `pos` ends. A character is what's seen as one, like an emoji. */
export function nextChar(text: string, pos: number): number {
  const end = lineEnd(text, pos);
  if (pos >= end) return Math.min(pos + 1, text.length);
  const start = lineStart(text, pos);
  for (const { index, segment } of graphemes.segment(text.slice(start, end))) {
    const after = start + index + segment.length;
    if (after > pos) return after;
  }
  return end;
}

/** Where the character before `pos` starts. */
export function prevChar(text: string, pos: number): number {
  const start = lineStart(text, pos);
  if (pos <= start) return Math.max(pos - 1, 0);
  let last = start;
  for (const { index } of graphemes.segment(text.slice(start, pos))) last = start + index;
  return last;
}

/** The furthest the cursor goes in normal mode: onto the last character, not past it. */
function lastColumn(text: string, pos: number): number {
  const start = lineStart(text, pos);
  const end = lineEnd(text, pos);
  return end > start ? prevChar(text, end) : start;
}

/** A place the normal mode cursor can be, near `pos`. */
export function normalCursor(text: string, pos: number): number {
  const at = Math.max(0, Math.min(pos, text.length));
  return Math.min(at, lastColumn(text, at));
}

function lineIndex(text: string, pos: number): number {
  let index = 0;
  for (let at = text.indexOf("\n"); at >= 0 && at < pos; at = text.indexOf("\n", at + 1)) index++;
  return index;
}

const lineCount = (text: string) => lineIndex(text, text.length) + 1;

function startOfLine(text: string, index: number): number {
  let pos = 0;
  for (let line = 0; line < index; line++) {
    const end = text.indexOf("\n", pos);
    if (end < 0) return pos;
    pos = end + 1;
  }
  return pos;
}

/** Where a character ends when a range includes it. A line's end includes its line break. */
const charEnd = (text: string, pos: number) =>
  pos >= text.length ? pos : text[pos] === "\n" ? pos + 1 : nextChar(text, pos);

// ----- Words -----

/** Vim's kinds of character: blank, punctuation, and letters, digits and underscores. With
 * `big`, everything that isn't blank is one kind, which is what W, B and E move by. */
function charKind(ch: string | undefined, big: boolean): number {
  if (ch === undefined || /\s/.test(ch)) return 0;
  if (big) return 1;
  return /[\p{L}\p{N}_]/u.test(ch) ? 2 : 1;
}

function wordForward(text: string, pos: number, big: boolean): number {
  const n = text.length;
  let at = pos;
  const kind = charKind(text[at], big);
  if (kind !== 0) while (at < n && charKind(text[at], big) === kind) at++;
  while (at < n && charKind(text[at], big) === 0) {
    // An empty line counts as a word.
    if (text[at] === "\n" && text[at + 1] === "\n") return at + 1;
    at++;
  }
  return at;
}

function wordEnd(text: string, pos: number, big: boolean): number {
  const n = text.length;
  let at = pos + 1;
  while (at < n && charKind(text[at], big) === 0) at++;
  if (at >= n) return Math.max(pos, n - 1);
  const kind = charKind(text[at], big);
  while (at + 1 < n && charKind(text[at + 1], big) === kind) at++;
  return at;
}

function wordBack(text: string, pos: number, big: boolean): number {
  let at = pos - 1;
  while (at > 0 && charKind(text[at], big) === 0) {
    if (text[at] === "\n" && text[at - 1] === "\n") return at;
    at--;
  }
  if (at <= 0) return 0;
  const kind = charKind(text[at], big);
  while (at > 0 && charKind(text[at - 1], big) === kind) at--;
  return at;
}

/** The last character of the run the cursor is in, for cw. */
function runEnd(text: string, pos: number, big: boolean): number {
  const kind = charKind(text[pos], big);
  let at = pos;
  while (at + 1 < text.length && charKind(text[at + 1], big) === kind) at++;
  return at;
}

// ----- Motions -----

interface Motion {
  pos: number;
  linewise?: boolean;
  /** The character it lands on is part of the range, like e and f. */
  inclusive?: boolean;
  column?: number;
}

function findInLine(
  text: string,
  cursor: number,
  kind: FindKind,
  char: string,
  count: number,
  repeat: boolean,
): number | null {
  const start = lineStart(text, cursor);
  const end = lineEnd(text, cursor);
  let at = cursor;
  for (let step = 0; step < count; step++) {
    // Repeating t from just before the character would find the same one, so it looks past.
    const skip = (kind === "t" || kind === "T") && (repeat || step > 0) ? 1 : 0;
    if (kind === "f" || kind === "t") {
      const found = text.indexOf(char, at + 1 + skip);
      if (found < 0 || found >= end) return null;
      at = kind === "f" ? found : found - 1;
    } else {
      const from = at - 1 - skip;
      if (from < start) return null;
      const found = text.lastIndexOf(char, from);
      if (found < start) return null;
      at = kind === "F" ? found : found + 1;
    }
  }
  return at;
}

function moveLines(text: string, cursor: number, by: number, state: VimState, operator: boolean): Motion | null {
  const line = lineIndex(text, cursor);
  const target = Math.max(0, Math.min(line + by, lineCount(text) - 1));
  if (target === line) return null;
  const column = state.column ?? cursor - lineStart(text, cursor);
  const start = startOfLine(text, target);
  const limit = operator ? lineEnd(text, start) : lastColumn(text, start);
  return { pos: Math.min(start + column, limit), linewise: true, column };
}

function motion(
  text: string,
  cursor: number,
  command: Command,
  state: VimState,
  operator: boolean,
): Motion | null {
  const { count } = command;
  const line = lineIndex(text, cursor);
  switch (command.key) {
    case "h":
    case "<Left>":
    case "<BS>": {
      const start = lineStart(text, cursor);
      let at = cursor;
      for (let step = 0; step < count && at > start; step++) at = prevChar(text, at);
      return { pos: at };
    }
    case "l":
    case "<Right>":
    case " ": {
      const limit = operator ? lineEnd(text, cursor) : lastColumn(text, cursor);
      let at = cursor;
      for (let step = 0; step < count && at < limit; step++) at = nextChar(text, at);
      return { pos: Math.min(at, limit) };
    }
    case "j":
    case "<Down>":
      return moveLines(text, cursor, count, state, operator);
    case "k":
    case "<Up>":
      return moveLines(text, cursor, -count, state, operator);
    case "w":
    case "W": {
      const big = command.key === "W";
      let at = cursor;
      for (let step = 0; step < count; step++) {
        const before = at;
        at = wordForward(text, at, big);
        // dw on a line's last word stops at the line's end rather than taking the break.
        if (operator && step === count - 1) {
          const lineBreak = text.indexOf("\n", before);
          if (lineBreak >= 0 && lineBreak < at) at = lineBreak;
        }
      }
      return { pos: at };
    }
    case "e":
    case "E": {
      let at = cursor;
      for (let step = 0; step < count; step++) at = wordEnd(text, at, command.key === "E");
      return { pos: at, inclusive: true };
    }
    case "b":
    case "B": {
      let at = cursor;
      for (let step = 0; step < count; step++) at = wordBack(text, at, command.key === "B");
      return { pos: at };
    }
    case "0":
    case "<Home>":
      return { pos: lineStart(text, cursor) };
    case "^":
      return { pos: firstNonBlank(text, cursor) };
    case "$":
    case "<End>": {
      const start = startOfLine(text, Math.min(line + count - 1, lineCount(text) - 1));
      return operator
        ? { pos: lineEnd(text, start) }
        : { pos: lastColumn(text, start), column: Number.POSITIVE_INFINITY };
    }
    case "G":
    case "gg": {
      const last = lineCount(text) - 1;
      const fallback = command.key === "G" ? last : 0;
      const target = command.hasCount ? Math.min(count - 1, last) : fallback;
      return { pos: firstNonBlank(text, startOfLine(text, target)), linewise: true };
    }
    case "f":
    case "t":
    case "F":
    case "T":
    case ";":
    case ",": {
      let kind: FindKind;
      let char: string;
      let repeat = false;
      if (command.key === ";" || command.key === ",") {
        if (!state.lastFind) return null;
        ({ kind, char } = state.lastFind);
        repeat = true;
        if (command.key === ",") kind = REVERSED[kind];
      } else {
        kind = command.key;
        char = command.char ?? "";
      }
      const pos = findInLine(text, cursor, kind, char, count, repeat);
      if (pos === null) return null;
      return { pos, inclusive: kind === "f" || kind === "t" };
    }
  }
  return null;
}

const REVERSED: Record<FindKind, FindKind> = { f: "F", F: "f", t: "T", T: "t" };

// ----- Text objects -----

const BRACKETS: Record<string, [string, string]> = {
  "(": ["(", ")"],
  ")": ["(", ")"],
  b: ["(", ")"],
  "[": ["[", "]"],
  "]": ["[", "]"],
  "{": ["{", "}"],
  "}": ["{", "}"],
  B: ["{", "}"],
  "<": ["<", ">"],
  ">": ["<", ">"],
};
const QUOTES = new Set(['"', "'", "`"]);
const OBJECTS = new Set(["w", "W", ...QUOTES, ...Object.keys(BRACKETS)]);

function wordObject(text: string, cursor: number, around: boolean, big: boolean) {
  const start = lineStart(text, cursor);
  const end = lineEnd(text, cursor);
  if (start === end) return null;
  const kind = charKind(text[cursor], big);
  let from = cursor;
  let to = cursor + 1;
  while (from > start && charKind(text[from - 1], big) === kind) from--;
  while (to < end && charKind(text[to], big) === kind) to++;
  if (!around) return { from, to };
  if (kind === 0) {
    // On blanks, aw is the blanks and the word after them.
    const next = charKind(text[to], big);
    while (to < end && charKind(text[to], big) === next) to++;
    return { from, to };
  }
  const word = to;
  while (to < end && isBlank(text[to])) to++;
  if (to === word) while (from > start && isBlank(text[from - 1])) from--;
  return { from, to };
}

function quoteObject(text: string, cursor: number, around: boolean, quote: string) {
  const start = lineStart(text, cursor);
  const end = lineEnd(text, cursor);
  const quotes: number[] = [];
  for (let at = start; at < end; at++) {
    if (text[at] === quote && text[at - 1] !== "\\") quotes.push(at);
  }
  for (let pair = 0; pair + 1 < quotes.length; pair += 2) {
    const open = quotes[pair];
    const close = quotes[pair + 1];
    // The pair around the cursor, or else the first one after it, the way Vim looks ahead.
    if (close < cursor) continue;
    if (!around) return { from: open + 1, to: close };
    let from = open;
    let to = close + 1;
    while (to < end && isBlank(text[to])) to++;
    if (to === close + 1) while (from > start && isBlank(text[from - 1])) from--;
    return { from, to };
  }
  return null;
}

function bracketObject(text: string, cursor: number, around: boolean, [open, close]: [string, string]) {
  let depth = 0;
  let opened = -1;
  for (let at = Math.min(cursor, text.length - 1); at >= 0; at--) {
    if (text[at] === close && at !== cursor) depth++;
    else if (text[at] === open) {
      if (depth === 0) {
        opened = at;
        break;
      }
      depth--;
    }
  }
  if (opened < 0) return null;
  depth = 0;
  let closed = -1;
  for (let at = opened + 1; at < text.length; at++) {
    if (text[at] === open) depth++;
    else if (text[at] === close) {
      if (depth === 0) {
        closed = at;
        break;
      }
      depth--;
    }
  }
  if (closed < 0) return null;
  if (around) return { from: opened, to: closed + 1 };
  // A block written over several lines keeps the lines its brackets are on.
  let from = opened + 1;
  let to = closed;
  if (text[from] === "\n") from++;
  const closeLine = lineStart(text, closed);
  if (closeLine > from && text.slice(closeLine, closed).trim() === "") to = closeLine;
  return { from: Math.min(from, to), to };
}

function textObject(text: string, cursor: number, around: boolean, char: string) {
  if (char === "w" || char === "W") return wordObject(text, cursor, around, char === "W");
  if (QUOTES.has(char)) return quoteObject(text, cursor, around, char);
  const pair = BRACKETS[char];
  return pair ? bracketObject(text, cursor, around, pair) : null;
}

// ----- Reading a command -----

type Operator = "d" | "c" | "y";

interface Command {
  /** Both counts multiplied, as in 2d3w. 1 when there's none. */
  count: number;
  hasCount: boolean;
  op: Operator | null;
  /** A motion, an action, "line" for dd, cc and yy, or "object" for iw and the like. */
  key: string;
  /** What f, t and r take, or the kind of text object. */
  char?: string;
  around?: boolean;
}

const MOTIONS = new Set([
  "h", "j", "k", "l", "w", "b", "e", "W", "B", "E", "0", "^", "$", "G", ";", ",",
  "<Left>", "<Right>", "<Up>", "<Down>", "<Home>", "<End>", "<BS>", " ",
]);
const FINDS = new Set(["f", "t", "F", "T"]);
const NORMAL_ACTIONS = new Set([
  "x", "X", "D", "C", "s", "S", "Y", "i", "a", "I", "A", "o", "O", "p", "P", "u", "<C-r>",
  "J", "~", "v", "V", "<Del>",
]);
const VISUAL_ACTIONS = new Set([
  "d", "x", "X", "D", "y", "Y", "c", "s", "S", "C", "p", "P", "J", "~", "o", "v", "V",
  "<Esc>", "<C-[>", "<Del>",
]);

const isOperator = (key: string): key is Operator => key === "d" || key === "c" || key === "y";

/** A key that types a character, rather than a named one like <Esc>. */
const isChar = (key: string) => !/^<.+>$/.test(key);

function parse(keys: string[], visual: boolean): Command | "more" | "bad" {
  let at = 0;
  const readCount = () => {
    let digits = "";
    // 0 on its own goes to the start of the line, and only counts after another digit.
    while (at < keys.length && /^[0-9]$/.test(keys[at]) && (digits !== "" || keys[at] !== "0")) {
      digits += keys[at++];
    }
    return digits === "" ? null : Number(digits);
  };
  const first = readCount();
  if (at === keys.length) return "more";
  let key = keys[at++];
  let op: Operator | null = null;
  let second: number | null = null;
  if (!visual && isOperator(key)) {
    op = key;
    second = readCount();
    if (at === keys.length) return "more";
    key = keys[at++];
  }
  const make = (fields: Partial<Command> = {}): Command => ({
    count: Math.min((first ?? 1) * (second ?? 1), 10_000),
    hasCount: first !== null || second !== null,
    op,
    key,
    ...fields,
  });
  // The next key, if there is one yet.
  const next = at < keys.length ? keys[at] : null;
  if (op && key === op) return make({ key: "line" });
  if ((op || visual) && (key === "i" || key === "a")) {
    if (next === null) return "more";
    return OBJECTS.has(next) ? make({ key: "object", char: next, around: key === "a" }) : "bad";
  }
  if (FINDS.has(key) || (key === "r" && !op && !visual)) {
    if (next === null) return "more";
    return isChar(next) ? make({ char: next }) : "bad";
  }
  if (key === "g") {
    if (next === null) return "more";
    return next === "g" ? make({ key: "gg" }) : "bad";
  }
  if (MOTIONS.has(key)) return make();
  if (op) return "bad";
  return (visual ? VISUAL_ACTIONS : NORMAL_ACTIONS).has(key) ? make() : "bad";
}

// ----- Running a command -----

interface Range {
  from: number;
  to: number;
  linewise: boolean;
}

const toNormal = (state: VimState): VimState => ({ ...state, mode: "normal", pending: [] });
const toInsert = (state: VimState): VimState => ({ ...state, mode: "insert", pending: [], typed: null });

const applied = (text: string, edit: Edit) =>
  text.slice(0, edit.from) + edit.insert + text.slice(edit.to);

/** Deletes, changes or yanks a range. `cursor` is where a yank leaves it. */
function operate(op: Operator, range: Range, text: string, cursor: number, state: VimState): VimResult {
  const from = Math.min(range.from, range.to);
  const to = Math.max(range.from, range.to);
  if (range.linewise) {
    const start = lineStart(text, from);
    const end = lineEnd(text, to);
    const yank = { text: `${text.slice(start, end)}\n`, linewise: true };
    if (op === "y") {
      const at = start < lineStart(text, cursor) ? normalCursor(text, start + cursor - lineStart(text, cursor)) : cursor;
      return { state: toNormal(state), handled: true, cursor: at, yank };
    }
    if (op === "c") {
      return { state: toInsert(state), handled: true, cursor: start, edit: { from: start, to: end, insert: "" }, yank };
    }
    // A line goes with its break, or the one before it when it's the last line.
    const edit = { from: start, to: end, insert: "" };
    if (end < text.length) edit.to = end + 1;
    else if (start > 0) edit.from = start - 1;
    const next = applied(text, edit);
    const landed = edit.from === start ? start : lineStart(next, edit.from);
    const result = { state: toNormal(state), handled: true, cursor: firstNonBlank(next, landed), yank };
    return edit.from < edit.to ? { ...result, edit } : result;
  }
  if (from === to) {
    return { state: op === "c" ? toInsert(state) : toNormal(state), handled: true, cursor: from };
  }
  const yank = { text: text.slice(from, to), linewise: false };
  if (op === "y") return { state: toNormal(state), handled: true, cursor: normalCursor(text, from), yank };
  const edit = { from, to, insert: "" };
  if (op === "c") return { state: toInsert(state), handled: true, cursor: from, edit, yank };
  return { state: toNormal(state), handled: true, cursor: normalCursor(applied(text, edit), from), edit, yank };
}

/** The range of a motion from the cursor, for an operator. */
function motionRange(text: string, cursor: number, found: Motion): Range {
  const from = Math.min(cursor, found.pos);
  const last = Math.max(cursor, found.pos);
  return { from, to: found.inclusive ? charEnd(text, last) : last, linewise: found.linewise ?? false };
}

function put(text: string, cursor: number, register: Register, before: boolean, count: number) {
  const times = Math.min(count, 1000);
  if (register.linewise) {
    const body = Array<string>(times).fill(register.text.replace(/\n$/, "")).join("\n");
    if (before) {
      const start = lineStart(text, cursor);
      const edit = { from: start, to: start, insert: `${body}\n` };
      return { edit, cursor: firstNonBlank(applied(text, edit), start) };
    }
    const end = lineEnd(text, cursor);
    const edit = { from: end, to: end, insert: `\n${body}` };
    return { edit, cursor: firstNonBlank(applied(text, edit), end + 1) };
  }
  const insert = register.text.repeat(times);
  const at = before || cursor >= lineEnd(text, cursor) ? cursor : nextChar(text, cursor);
  const edit = { from: at, to: at, insert };
  const next = applied(text, edit);
  return { edit, cursor: insert.includes("\n") ? at : prevChar(next, at + insert.length) };
}

/** Joins `lines` lines into the cursor's, with a space between unless one has nothing to join. */
function join(text: string, cursor: number, lines: number) {
  const from = lineEnd(text, cursor);
  if (from >= text.length) return null;
  const start = lineStart(text, cursor);
  let insert = "";
  let joinedAt = from;
  let to = from;
  for (let line = 1; line < lines && to < text.length; line++) {
    let next = to + 1;
    while (next < text.length && isBlank(text[next])) next++;
    const end = lineEnd(text, next);
    const body = text.slice(next, end);
    const before = (text.slice(start, from) + insert).at(-1);
    const space = body === "" || body.startsWith(")") || before === undefined || isBlank(before) ? "" : " ";
    joinedAt = from + insert.length;
    insert += space + body;
    to = end;
  }
  const edit = { from, to, insert };
  return { edit, cursor: normalCursor(applied(text, edit), joinedAt) };
}

const toggleCase = (text: string) =>
  [...text].map((ch) => (ch === ch.toUpperCase() ? ch.toLowerCase() : ch.toUpperCase())).join("");

/** What visual mode has selected. */
function selected(state: VimState, text: string, cursor: number): Range {
  const from = Math.min(state.anchor, cursor);
  const last = Math.max(state.anchor, cursor);
  if (state.mode === "visual_line") return { from, to: last, linewise: true };
  return { from, to: charEnd(text, last), linewise: false };
}

/** The lines a range covers, from the start of the first to the end of the last. */
function lineSpan(text: string, range: Range) {
  return { start: lineStart(text, range.from), end: lineEnd(text, range.to) };
}

function runVisual(command: Command, state: VimState, input: VimInput): VimResult {
  const { text, cursor, register } = input;
  const range = selected(state, text, cursor);
  const done = (result: Partial<VimResult> & { cursor: number }): VimResult => ({
    state: toNormal(state),
    handled: true,
    ...result,
  });
  // A selection that ends on a line break takes the break, but not the line after it.
  const lastSelected = range.linewise ? range.to : Math.max(range.from, range.to - 1);
  const lines = { from: range.from, to: lastSelected, linewise: true };
  switch (command.key) {
    case "d":
    case "x":
    case "<Del>":
      return operate("d", range, text, range.from, state);
    case "X":
    case "D":
      return operate("d", lines, text, range.from, state);
    case "y":
      return operate("y", range, text, range.from, state);
    case "Y":
      return operate("y", lines, text, range.from, state);
    case "c":
    case "s":
      return operate("c", range, text, range.from, state);
    case "S":
    case "C":
      return operate("c", lines, text, range.from, state);
    case "p":
    case "P": {
      if (!register.text) return done({ cursor: normalCursor(text, cursor) });
      const body = register.text.replace(/\n$/, "");
      let edit: Edit;
      let yank: Register;
      if (range.linewise) {
        const { start, end } = lineSpan(text, range);
        edit = { from: start, to: end, insert: register.linewise ? body : register.text };
        yank = { text: `${text.slice(start, end)}\n`, linewise: true };
      } else {
        edit = { from: range.from, to: range.to, insert: register.linewise ? `\n${body}\n` : register.text };
        yank = { text: text.slice(range.from, range.to), linewise: false };
      }
      const next = applied(text, edit);
      const at = range.linewise ? firstNonBlank(next, edit.from) : normalCursor(next, edit.from);
      return done({ cursor: at, edit, yank });
    }
    case "J": {
      const count = lineIndex(text, lastSelected) - lineIndex(text, range.from) + 1;
      const joined = join(text, range.from, Math.max(count, 2));
      return done(joined ?? { cursor: normalCursor(text, cursor) });
    }
    case "~": {
      const span = lineSpan(text, range);
      const { from, to } = range.linewise ? { from: span.start, to: span.end } : range;
      const edit = { from, to, insert: toggleCase(text.slice(from, to)) };
      return done({ cursor: normalCursor(text, from), edit });
    }
    case "o":
      return { state: { ...state, pending: [], anchor: cursor }, handled: true, cursor: state.anchor };
    case "v":
    case "V": {
      const mode = command.key === "v" ? "visual" : "visual_line";
      if (state.mode === mode) return done({ cursor: normalCursor(text, cursor) });
      return { state: { ...state, mode, pending: [] }, handled: true, cursor };
    }
    case "<Esc>":
    case "<C-[>":
      return done({ cursor: normalCursor(text, cursor) });
    case "object": {
      const found = textObject(text, cursor, command.around ?? false, command.char ?? "");
      if (!found || found.from === found.to) return { state: { ...state, pending: [] }, handled: true, cursor };
      return {
        state: { ...state, mode: "visual", pending: [], anchor: found.from },
        handled: true,
        cursor: Math.max(found.from, prevChar(text, found.to)),
      };
    }
  }
  const found = motion(text, cursor, command, state, false);
  const moved = { ...state, pending: [], lastFind: findOf(command, state), column: found?.column ?? null };
  return { state: moved, handled: true, cursor: found ? normalCursor(text, found.pos) : cursor };
}

/** What ; and , repeat after this command. */
function findOf(command: Command, state: VimState): VimState["lastFind"] {
  const { key, char } = command;
  if ((key === "f" || key === "t" || key === "F" || key === "T") && char) return { kind: key, char };
  return state.lastFind;
}

function runNormal(command: Command, state: VimState, input: VimInput): VimResult {
  const { text, cursor, register } = input;
  const { count } = command;
  const base = { ...state, pending: [] };
  const stay: VimResult = { state: base, handled: true, cursor };
  const start = lineStart(text, cursor);
  const end = lineEnd(text, cursor);
  const line = lineIndex(text, cursor);
  const lastLine = () => startOfLine(text, Math.min(line + count - 1, lineCount(text) - 1));

  if (command.op) {
    const { op } = command;
    if (command.key === "line") {
      return operate(op, { from: cursor, to: lastLine(), linewise: true }, text, cursor, base);
    }
    if (command.key === "object") {
      const found = textObject(text, cursor, command.around ?? false, command.char ?? "");
      return found ? operate(op, { ...found, linewise: false }, text, cursor, base) : stay;
    }
    // cw on a word changes to its end and leaves the space after it.
    const big = command.key === "W";
    if (op === "c" && (command.key === "w" || big) && charKind(text[cursor], big) !== 0) {
      let at = cursor;
      for (let step = 0; step < count; step++) at = step === 0 ? runEnd(text, at, big) : wordEnd(text, at, big);
      return operate(op, { from: cursor, to: charEnd(text, at), linewise: false }, text, cursor, base);
    }
    const found = motion(text, cursor, command, state, true);
    if (!found) return stay;
    const withFind = { ...base, lastFind: findOf(command, state) };
    return operate(op, motionRange(text, cursor, found), text, cursor, withFind);
  }

  switch (command.key) {
    case "x":
    case "<Del>":
    case "s": {
      let to = cursor;
      for (let step = 0; step < count && to < end; step++) to = nextChar(text, to);
      return operate(command.key === "s" ? "c" : "d", { from: cursor, to, linewise: false }, text, cursor, base);
    }
    case "X": {
      let from = cursor;
      for (let step = 0; step < count && from > start; step++) from = prevChar(text, from);
      return operate("d", { from, to: cursor, linewise: false }, text, cursor, base);
    }
    case "D":
    case "C":
      return operate(command.key === "D" ? "d" : "c", { from: cursor, to: lineEnd(text, lastLine()), linewise: false }, text, cursor, base);
    case "S":
      return operate("c", { from: cursor, to: lastLine(), linewise: true }, text, cursor, base);
    case "Y":
      return operate("y", { from: cursor, to: lastLine(), linewise: true }, text, cursor, base);
    case "r": {
      const char = command.char ?? "";
      let to = cursor;
      for (let step = 0; step < count; step++) {
        if (to >= end) return stay;
        to = nextChar(text, to);
      }
      const insert = char.repeat(count);
      return { state: base, handled: true, cursor: cursor + insert.length - char.length, edit: { from: cursor, to, insert } };
    }
    case "i":
      return { state: toInsert(base), handled: true, cursor };
    case "a":
      return { state: toInsert(base), handled: true, cursor: cursor < end ? nextChar(text, cursor) : cursor };
    case "I":
      return { state: toInsert(base), handled: true, cursor: firstNonBlank(text, cursor) };
    case "A":
      return { state: toInsert(base), handled: true, cursor: end };
    case "o":
      return { state: toInsert(base), handled: true, cursor: end + 1, edit: { from: end, to: end, insert: "\n" } };
    case "O":
      return { state: toInsert(base), handled: true, cursor: start, edit: { from: start, to: start, insert: "\n" } };
    case "p":
    case "P": {
      if (!register.text) return stay;
      return { state: base, handled: true, ...put(text, cursor, register, command.key === "P", count) };
    }
    case "u":
      return { state: base, handled: true, cursor, history: { action: "undo", count } };
    case "<C-r>":
      return { state: base, handled: true, cursor, history: { action: "redo", count } };
    case "J": {
      const joined = join(text, cursor, Math.max(count, 2));
      return joined ? { state: base, handled: true, ...joined } : stay;
    }
    case "~": {
      let to = cursor;
      for (let step = 0; step < count && to < end; step++) to = nextChar(text, to);
      if (to === cursor) return stay;
      const edit = { from: cursor, to, insert: toggleCase(text.slice(cursor, to)) };
      return { state: base, handled: true, cursor: normalCursor(applied(text, edit), to), edit };
    }
    case "v":
    case "V":
      return {
        state: { ...base, mode: command.key === "v" ? "visual" : "visual_line", anchor: cursor },
        handled: true,
        cursor,
      };
  }
  const found = motion(text, cursor, command, state, false);
  if (!found) return { ...stay, state: { ...base, lastFind: findOf(command, state) } };
  return {
    state: { ...base, column: found.column ?? null, lastFind: findOf(command, state) },
    handled: true,
    cursor: normalCursor(text, found.pos),
  };
}

/** Leaving insert mode steps back onto the character just typed, as Vim does. */
const leaveInsert = (text: string, cursor: number) =>
  normalCursor(text, cursor > lineStart(text, cursor) ? prevChar(text, cursor) : cursor);

function insertKey(state: VimState, key: string, input: VimInput, options: VimOptions, now: number): VimResult {
  const { text, cursor } = input;
  if (key === "<Esc>" || key === "<C-[>") {
    return { state: { ...toNormal(state), typed: null, column: null }, handled: true, cursor: leaveInsert(text, cursor) };
  }
  const escape = [...options.escape];
  const { typed } = state;
  if (
    escape.length === 2 &&
    key === escape[1] &&
    typed?.key === escape[0] &&
    now - typed.at <= options.timeoutMs &&
    text.slice(cursor - escape[0].length, cursor) === escape[0]
  ) {
    // The first letter was typed into the text while waiting to see what came next.
    const edit = { from: cursor - escape[0].length, to: cursor, insert: "" };
    return {
      state: { ...toNormal(state), typed: null, column: null },
      handled: true,
      cursor: leaveInsert(applied(text, edit), edit.from),
      edit,
    };
  }
  return { state: { ...state, typed: isChar(key) ? { key, at: now } : null }, handled: false, cursor };
}

/** Runs one key. Keys are what they type, or Vim's names for the rest: <Esc>, <C-r>, <BS>,
 * <Del>, <Left>, <Right>, <Up>, <Down>, <Home> and <End>. */
export function vimKey(state: VimState, key: string, input: VimInput, options: VimOptions, now: number): VimResult {
  if (state.mode === "insert") return insertKey(state, key, input, options, now);
  const visual = state.mode !== "normal";
  const keys = [...state.pending, key];
  const command = parse(keys, visual);
  if (command === "more") return { state: { ...state, pending: keys }, handled: true, cursor: input.cursor };
  if (command === "bad") {
    const cleared = { ...state, pending: [] };
    // Esc with nothing to cancel leaves the box, and Ctrl keys vim doesn't know keep theirs.
    const theirs = key === "<Esc>" || key.startsWith("<C-");
    return { state: cleared, handled: !(theirs && state.pending.length === 0), cursor: input.cursor };
  }
  return visual ? runVisual(command, state, input) : runNormal(command, state, input);
}

/** What the text box shows as selected: a block on the character in normal mode, the
 * selection in visual mode, and a plain caret in insert mode. */
export function selectionOf(state: VimState, text: string, cursor: number) {
  const backward = cursor < state.anchor;
  switch (state.mode) {
    case "insert":
      return { start: cursor, end: cursor, backward: false };
    case "normal":
      return { start: cursor, end: cursor < lineEnd(text, cursor) ? nextChar(text, cursor) : cursor, backward: false };
    case "visual": {
      const range = selected(state, text, cursor);
      return { start: range.from, end: range.to, backward };
    }
    case "visual_line": {
      const { start, end } = lineSpan(text, selected(state, text, cursor));
      return { start, end, backward };
    }
  }
}
