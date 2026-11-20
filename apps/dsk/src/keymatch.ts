// Comparing a key press with a shortcut from the settings file, and showing one. No DOM
// or Tauri in here, so it can be tested on its own. Shortcuts arrive in the settings
// crate's canonical form: modifiers in a fixed order, then the key, all lower case.

/** One key for an action, and how it has to be pressed. */
export interface Binding {
  key: string;
  /** How long it has to be held before the action runs. 0 runs it on press. */
  hold_ms: number;
  /** Whether keeping it pressed runs the action again and again. */
  repeat: boolean;
}

export interface KeyPress {
  key: string;
  code: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

const MODIFIER_KEYS = new Set(["Meta", "Control", "Alt", "Shift", "CapsLock", "Fn", "Dead", "Unidentified"]);

const KEY_NAMES: Record<string, string> = {
  arrowup: "up",
  arrowdown: "down",
  arrowleft: "left",
  arrowright: "right",
  " ": "space",
  "+": "plus",
};

const nameOf = (key: string): string => {
  const lower = key.toLowerCase();
  return KEY_NAMES[lower] ?? lower;
};

/** The key without what holding Shift or Option turned it into: Option+K types "˚" on
 * macOS. This is the physical key, so it's only right on layouts where that matches the
 * letter printed on it. */
function physicalKey(code: string): string | undefined {
  return /^Key([A-Z])$/.exec(code)?.[1].toLowerCase() ?? /^Digit(\d)$/.exec(code)?.[1];
}

/** A character only reachable with Shift on some layouts, like "/" on a German keyboard. */
const isSymbol = (key: string) => [...key].length === 1 && !/[\p{L}\p{N}]/u.test(key);

function parts(chord: string) {
  const all = chord.split("+");
  const key = all[all.length - 1];
  const modifiers = new Set(all.slice(0, -1));
  return { key, modifiers };
}

export function chordMatches(chord: string, press: KeyPress, mac: boolean): boolean {
  const { key, modifiers } = parts(chord);
  const wantsMod = modifiers.has("mod");
  const meta = modifiers.has("cmd") || (wantsMod && mac);
  const ctrl = modifiers.has("ctrl") || (wantsMod && !mac);
  if (press.metaKey !== meta || press.ctrlKey !== ctrl || press.altKey !== modifiers.has("alt")) {
    return false;
  }
  const wantsShift = modifiers.has("shift");
  const pressed = nameOf(press.key);
  if (pressed === key) {
    return wantsShift ? press.shiftKey : !press.shiftKey || isSymbol(key);
  }
  const held = press.altKey || press.shiftKey;
  return held && press.shiftKey === wantsShift && physicalKey(press.code) === key;
}

/** The shortcut a press would be recorded as, or null for a modifier on its own. */
export function chordOf(press: KeyPress, mac: boolean): string | null {
  if (MODIFIER_KEYS.has(press.key)) return null;
  const typed = nameOf(press.key);
  const key = press.altKey ? (physicalKey(press.code) ?? typed) : typed;
  const out: string[] = [];
  const mod = mac ? press.metaKey : press.ctrlKey;
  const other = mac ? press.ctrlKey : press.metaKey;
  if (mod) out.push("mod");
  if (other && !mac) out.push("cmd");
  if (other && mac) out.push("ctrl");
  if (press.altKey) out.push("alt");
  // Shift is already part of a symbol: pressing it for "?" records "?", not "shift+?".
  if (press.shiftKey && !isSymbol(key)) out.push("shift");
  out.push(key);
  return out.join("+");
}

export const isSequence = (key: string) => key.includes(" ");

/** How a sequence stands after these presses, the latest last: it's complete, it could
 * still be completed by the next key, or neither. Only its end has to line up, so keys
 * that came before it and led nowhere don't matter, the same as in Vim. */
export function sequenceState(
  key: string,
  pressed: KeyPress[],
  mac: boolean,
): "full" | "prefix" | null {
  const steps = key.split(" ");
  const lineUp = (count: number) => {
    const tail = pressed.slice(pressed.length - count);
    return tail.every((press, index) => chordMatches(steps[index], press, mac));
  };
  if (pressed.length >= steps.length && lineUp(steps.length)) return "full";
  for (let count = Math.min(steps.length - 1, pressed.length); count >= 1; count--) {
    if (lineUp(count)) return "prefix";
  }
  return null;
}

/** Types a character into a text field when pressed there, unless it has a modifier. */
export function isBareChord(chord: string): boolean {
  const { key, modifiers } = parts(chord);
  return (
    !/^f\d+$/.test(key) && !modifiers.has("mod") && !modifiers.has("cmd") && !modifiers.has("ctrl") && !modifiers.has("alt")
  );
}

const GLYPHS: Record<string, string> = {
  up: "↑",
  down: "↓",
  left: "←",
  right: "→",
  enter: "↵",
  escape: "Esc",
  backspace: "⌫",
  delete: "⌦",
  tab: "⇥",
  space: "Space",
  home: "Home",
  end: "End",
  pageup: "PgUp",
  pagedown: "PgDn",
  plus: "+",
};

const keyLabel = (key: string) => GLYPHS[key] ?? key.toUpperCase();

/** A sequence reads the way Vim writes it, so "g shift+t" is gT, while a single key
 * reads like what's printed on the keyboard. */
export function formatSequence(key: string, mac: boolean): string {
  if (!isSequence(key)) return formatChord(key, mac);
  const steps = key.split(" ").map((step) => {
    if (/^[a-z]$/.test(step)) return step;
    const capital = /^shift\+([a-z])$/.exec(step)?.[1];
    return capital ? capital.toUpperCase() : formatChord(step, mac);
  });
  return steps.join(steps.every((step) => step.length === 1) ? "" : " ");
}

export function formatChord(chord: string, mac: boolean): string {
  const { key, modifiers } = parts(chord);
  const names: [string, string, string][] = [
    ["mod", "⌘", "Ctrl"],
    ["cmd", "⌘", "Win"],
    ["ctrl", "⌃", "Ctrl"],
    ["alt", "⌥", "Alt"],
    ["shift", "⇧", "Shift"],
  ];
  const held = names.filter(([name]) => modifiers.has(name));
  if (mac) return held.map(([, glyph]) => glyph).join("") + keyLabel(key);
  return [...held.map(([, , word]) => word), keyLabel(key)].join("+");
}

export function formatBinding(binding: Binding, mac: boolean): string {
  const label = formatSequence(binding.key, mac);
  return binding.hold_ms > 0 ? `Hold ${label}` : label;
}

/** Whether a key starts with Cmd. Terminals get every other key, so only these reach the app
 * from one. */
export function usesCommand(key: string): boolean {
  const modifiers = key.split(" ")[0].split("+");
  return modifiers.includes("mod") || modifiers.includes("cmd");
}
