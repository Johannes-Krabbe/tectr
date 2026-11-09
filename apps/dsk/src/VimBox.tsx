// Vim in a textarea. vim.ts decides what a key does, and this makes it happen through the
// text box's own editing commands, so undo, React's state and the daemon's draft all see
// ordinary edits.

import { invoke } from "@tauri-apps/api/core";
import {
  type ClipboardEvent,
  type FocusEvent,
  type KeyboardEvent,
  type RefObject,
  useEffect,
  useRef,
  useState,
} from "react";
import { claim, isClaimed, sequenceTimeout, type Settings, useSettings } from "./keybinds";
import {
  type Edit,
  initialState,
  normalCursor,
  prevChar,
  type Register,
  selectionOf,
  type VimMode,
  type VimState,
  vimKey,
} from "./vim";

function vimSettings(settings: Settings | null) {
  const read = (id: string) => settings?.appearance.fields.find((field) => field.id === id)?.value;
  const start: VimMode = read("vim.start") === "normal" ? "normal" : "insert";
  return {
    enabled: read("vim.enabled") === true,
    start,
    clipboard: read("vim.system_clipboard") !== false,
    escape: String(read("vim.escape") ?? ""),
  };
}

// One register for every box, like Vim's unnamed register across its windows.
let register: Register = { text: "", linewise: false };
let sharesClipboard = false;
/** What this app last put on the clipboard. Reading that back keeps whether it was lines. */
let written: string | null = null;

function yank(next: Register) {
  register = next;
  if (!sharesClipboard) return;
  written = next.text;
  // The register has it either way, so a clipboard that won't take it only costs other apps.
  invoke("write_clipboard", { text: next.text }).catch(() => {});
}

/** Takes in what was copied since, here or in another app. Reading takes a moment and p
 * can't wait for it, so this runs whenever the clipboard may have changed: coming back to
 * the window, copying, and going to a box. */
function readClipboard() {
  if (!sharesClipboard) return;
  invoke<string>("read_clipboard").then(
    (text) => {
      if (!text || text === written) return;
      written = null;
      register = { text, linewise: text.endsWith("\n") };
    },
    // A clipboard that can't be read leaves p with what vim yanked last.
    () => {},
  );
}

window.addEventListener("focus", readClipboard);
for (const type of ["copy", "cut"]) {
  // The clipboard has the copy only once the event is over.
  document.addEventListener(type, () => setTimeout(readClipboard));
}

const NAMED: Record<string, string> = {
  Escape: "<Esc>",
  Backspace: "<BS>",
  Delete: "<Del>",
  ArrowLeft: "<Left>",
  ArrowRight: "<Right>",
  ArrowUp: "<Up>",
  ArrowDown: "<Down>",
  Home: "<Home>",
  End: "<End>",
};

const MODIFIERS = new Set(["Shift", "Control", "Alt", "Meta", "CapsLock", "Fn", "Dead", "Unidentified"]);

/** A press in vim's terms, or null for what vim leaves to the box in every mode: Cmd keys,
 * and Enter and Tab, which go on sending, adding lines and moving on. */
function keyName(event: KeyboardEvent): string | null {
  const { key } = event;
  if (event.metaKey || key === "Enter" || key === "Tab") return null;
  if (event.ctrlKey) return key.length === 1 ? `<C-${key.toLowerCase()}>` : null;
  const named = NAMED[key];
  if (named) return named;
  // Option turns letters into other characters on macOS. Vim has no use for them, but
  // insert mode still types them.
  if (event.altKey) return "<A>";
  return [...key].length === 1 ? key : null;
}

/** Edits through the box's own commands, so it's a step for undo and React hears of it. */
function applyEdit(el: HTMLTextAreaElement, edit: Edit) {
  el.setSelectionRange(edit.from, edit.to);
  const done = edit.insert
    ? document.execCommand("insertText", false, edit.insert)
    : edit.from === edit.to || document.execCommand("delete");
  if (!done) {
    el.setRangeText(edit.insert, edit.from, edit.to);
    el.dispatchEvent(new Event("input", { bubbles: true }));
  }
}

const LABELS: Record<VimMode, string> = {
  insert: "Insert",
  normal: "Normal",
  visual: "Visual",
  visual_line: "Visual line",
};

export function VimModeLabel({ mode }: { mode: VimMode | null }) {
  return mode ? <span className="composer-mode code">{LABELS[mode]}</span> : null;
}

export interface VimBox {
  /** The mode to show, or null while vim is off or the box doesn't have the keyboard. */
  shown: VimMode | null;
  /** Runs a press through vim, and says whether vim took it. */
  onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>): boolean;
  props: {
    "data-vim"?: VimMode;
    onFocus(): void;
    onBlur(event: FocusEvent<HTMLTextAreaElement>): void;
    onSelect(): void;
    onPaste(event: ClipboardEvent<HTMLTextAreaElement>): void;
  };
}

/** Vim for a textarea, when the settings turn it on. */
export function useVim(input: RefObject<HTMLTextAreaElement | null>): VimBox {
  const options = vimSettings(useSettings());
  // Every box has the same settings, so whichever rendered last says the same.
  sharesClipboard = options.enabled && options.clipboard;
  const state = useRef<VimState>(initialState(options.start));
  // What vim last selected, and where it left the cursor in it.
  const last = useRef<{ start: number; end: number; cursor: number } | null>(null);
  const [mode, setMode] = useState<VimMode>(options.start);
  const [focused, setFocused] = useState(false);

  const show = (el: HTMLTextAreaElement, next: VimState, cursor: number) => {
    state.current = next;
    setMode(next.mode);
    const { start, end, backward } = selectionOf(next, el.value, cursor);
    el.setSelectionRange(start, end, backward ? "backward" : "forward");
    last.current = { start, end, cursor };
  };

  /** Where vim left the cursor, unless the mouse or typing has moved it since. */
  const cursorOf = (el: HTMLTextAreaElement): number => {
    const { selectionStart: start, selectionEnd: end } = el;
    if (last.current && start === last.current.start && end === last.current.end) {
      return last.current.cursor;
    }
    const backward = el.selectionDirection === "backward";
    const { mode: now } = state.current;
    if (now === "visual" || now === "visual_line") {
      const final = Math.max(start, prevChar(el.value, end));
      state.current = { ...state.current, anchor: backward ? final : start };
      return backward ? start : final;
    }
    return now === "insert" && !backward ? end : start;
  };

  // Turning vim on or off, or changing how it starts, starts every box over.
  useEffect(() => {
    const el = input.current;
    state.current = { ...initialState(options.start), lastFind: state.current.lastFind };
    setMode(options.start);
    if (!el || document.activeElement !== el) return;
    if (options.enabled) show(el, state.current, cursorOf(el));
    else el.setSelectionRange(el.selectionStart, el.selectionStart);
  }, [options.enabled, options.start]);

  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>): boolean => {
    if (!options.enabled || event.nativeEvent.isComposing || isClaimed(event)) return false;
    if (MODIFIERS.has(event.key)) return false;
    const el = event.currentTarget;
    const key = keyName(event);
    if (key === null) {
      // Sending or a Cmd key ends a command that was started.
      if (state.current.pending.length > 0) state.current = { ...state.current, pending: [] };
      return false;
    }
    const cursor = cursorOf(el);
    const result = vimKey(
      state.current,
      key,
      { text: el.value, cursor, register },
      { escape: options.escape, timeoutMs: sequenceTimeout() },
      Date.now(),
    );
    state.current = result.state;
    if (!result.handled) return false;
    event.preventDefault();
    claim(event);
    if (result.yank) yank(result.yank);
    if (result.edit) applyEdit(el, result.edit);
    let at = result.cursor;
    if (result.history) {
      for (let step = 0; step < result.history.count; step++) {
        if (!document.execCommand(result.history.action)) break;
      }
      at = normalCursor(el.value, el.selectionStart);
    }
    show(el, result.state, at);
    return true;
  };

  return {
    shown: options.enabled && focused ? mode : null,
    onKeyDown,
    props: {
      "data-vim": options.enabled ? mode : undefined,
      onFocus() {
        setFocused(true);
        readClipboard();
        const el = input.current;
        if (options.enabled && el) show(el, state.current, cursorOf(el));
      },
      onBlur(event) {
        setFocused(false);
        const to = event.relatedTarget;
        const toOverlay = to instanceof Element && to.closest(".sheet, .select-popup") !== null;
        // Coming back from elsewhere in the app starts over. Switching apps, or opening a
        // list or the palette from the box, comes back to the mode it was in.
        if (!document.hasFocus() || toOverlay) return;
        state.current = { ...initialState(options.start), lastFind: state.current.lastFind };
        setMode(options.start);
      },
      // A click puts a plain caret down, which normal mode shows as a block.
      onSelect() {
        const el = input.current;
        if (!options.enabled || !el || document.activeElement !== el) return;
        if (state.current.mode !== "normal" || el.selectionStart !== el.selectionEnd) return;
        show(el, state.current, normalCursor(el.value, el.selectionStart));
      },
      // Pasting with Cmd+V outside insert mode would type over the block or the selection,
      // so it goes in at the cursor, or over the selection and back to normal mode.
      onPaste(event) {
        const el = event.currentTarget;
        const { mode: now } = state.current;
        if (!options.enabled || now === "insert") return;
        const text = event.clipboardData.getData("text");
        event.preventDefault();
        if (!text) return;
        const cursor = cursorOf(el);
        const visual = now === "visual" || now === "visual_line";
        const range = visual ? { from: el.selectionStart, to: el.selectionEnd } : { from: cursor, to: cursor };
        applyEdit(el, { ...range, insert: text });
        const next: VimState = { ...state.current, mode: "normal", pending: [] };
        show(el, next, normalCursor(el.value, range.from + text.length - 1));
      },
    },
  };
}
