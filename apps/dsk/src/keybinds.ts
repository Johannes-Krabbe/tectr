// Every key the app responds to goes through here, so it can be changed in
// ~/.config/tectr/dsk.toml. The file and its defaults belong to crates/settings.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";
import { useSyncExternalStore } from "react";
import { inTerminal } from "./keys";
import {
  type Appearance,
  type AppearancePreview,
  type AppearanceValue,
  type InstalledFont,
  previewAppearance,
  previewedTheme,
  showAppearance,
  shownValue,
  systemIsDark,
} from "./appearance";
import {
  type Binding,
  chordMatches,
  chordOf,
  formatBinding,
  formatKeys,
  isBareChord,
  isSequence,
  type KeyPress,
  sequenceState,
  usesCommand,
} from "./keymatch";

export type { Binding };

// Mirrors `ACTIONS` in crates/settings/src/keybinds.rs, and a test there checks it.
export type ActionId =
  | "nav.up"
  | "nav.down"
  | "nav.input_up"
  | "nav.input_down"
  | "nav.left"
  | "nav.right"
  | "nav.input_first"
  | "nav.input_last"
  | "nav.first"
  | "nav.last"
  | "nav.half_up"
  | "nav.half_down"
  | "nav.page_up"
  | "nav.page_down"
  | "nav.center"
  | "nav.confirm"
  | "nav.back"
  | "global.palette"
  | "global.settings"
  | "global.new_topic"
  | "global.new_workspace"
  | "global.new_tab"
  | "global.next_request"
  | "global.interrupt"
  | "global.close_tab"
  | "global.toggle_sidebar"
  | "global.tab_1"
  | "global.tab_2"
  | "global.tab_3"
  | "global.tab_4"
  | "global.tab_5"
  | "global.tab_6"
  | "global.tab_7"
  | "global.tab_8"
  | "global.tab_9"
  | "global.prev_tab"
  | "global.next_tab"
  | "global.prev_topic"
  | "global.next_topic"
  | "global.prev_workspace"
  | "global.next_workspace"
  | "global.mark_all_read"
  | "global.toggle_vim"
  | "global.move_topic"
  | "global.archived_topics"
  | "global.pair_machine"
  | "global.add_device"
  | "global.topic_mcp"
  | "sidebar.up"
  | "sidebar.down"
  | "sidebar.first"
  | "sidebar.last"
  | "sidebar.half_up"
  | "sidebar.half_down"
  | "sidebar.center"
  | "sidebar.parent"
  | "sidebar.open"
  | "sidebar.confirm"
  | "sidebar.add"
  | "sidebar.rename"
  | "sidebar.archive"
  | "sidebar.toggle_unread"
  | "pane.up"
  | "pane.down"
  | "pane.first"
  | "pane.last"
  | "pane.half_up"
  | "pane.half_down"
  | "pane.page_up"
  | "pane.page_down"
  | "pane.compose"
  | "pane.toggle_steps"
  | "pane.toggle_turn"
  | "pane.back"
  | "pane.next_request"
  | "pane.pick_model"
  | "pane.pick_effort"
  | "pane.pick_mode"
  | "composer.send"
  | "composer.newline"
  | "composer.blur"
  | "composer.command_up"
  | "composer.command_down"
  | "composer.complete_command"
  | "permission.allow_once"
  | "permission.allow_always"
  | "permission.reject"
  | "permission.option_1"
  | "permission.option_2"
  | "permission.option_3"
  | "permission.option_4"
  | "permission.option_5"
  | "permission.option_6"
  | "permission.option_7"
  | "permission.option_8"
  | "permission.option_9"
  | "new_tab.up"
  | "new_tab.down"
  | "new_tab.confirm"
  | "new_tab.agent"
  | "new_tab.terminal"
  | "new_tab.git"
  | "new_tab.diff"
  | "new_topic.start"
  | "new_topic.newline"
  | "new_topic.cancel"
  | "new_topic.pick_workspace"
  | "new_topic.pick_place"
  | "new_topic.pick_provider"
  | "new_topic.pick_model"
  | "new_topic.pick_effort"
  | "new_topic.pick_mode"
  | "sheet.submit"
  | "sheet.cancel"
  | "sheet.confirm"
  | "sheet.up"
  | "sheet.down"
  | "sheet.complete"
  | "palette.up"
  | "palette.down"
  | "palette.confirm"
  | "palette.cancel"
  | "picker.up"
  | "picker.down"
  | "picker.first"
  | "picker.last"
  | "picker.confirm"
  | "picker.cancel"
  | "device.confirm"
  | "device.remove"
  | "device.pick_model"
  | "device.pick_effort"
  | "device.rename"
  | "device.toggle_keep_awake"
  | "mcp.up"
  | "mcp.down"
  | "mcp.first"
  | "mcp.last"
  | "mcp.edit"
  | "mcp.back"
  | "mcp.add"
  | "mcp.remove"
  | "mcp.toggle"
  | "mcp.turn_on"
  | "mcp.turn_off"
  | "mcp.follow_default"
  | "mcp.login"
  | "mcp.logout"
  | "mcp.import"
  | "pairing.add"
  | "pairing.add_device"
  | "pairing.remove"
  | "pairing.forget"
  | "mcp_form.save"
  | "mcp_form.switch_type"
  | "mcp_form.toggle_enabled"
  | "sections.up"
  | "sections.down"
  | "sections.first"
  | "sections.last"
  | "sections.open"
  | "sections.confirm"
  | "sections.close"
  | "settings.up"
  | "settings.down"
  | "settings.first"
  | "settings.last"
  | "settings.half_up"
  | "settings.half_down"
  | "settings.center"
  | "settings.edit"
  | "settings.back"
  | "settings.parent"
  | "settings.add"
  | "settings.unbind"
  | "settings.reset"
  | "settings.decrease"
  | "settings.increase"
  | "settings.toggle_hold"
  | "settings.toggle_repeat"
  | "recorder.save"
  | "recorder.cancel";

export interface KeyAction {
  id: ActionId;
  group: string;
  label: string;
  bindings: Binding[];
  customized: boolean;
  inherits: ActionId | null;
  repeat: boolean;
}

export interface KeyGroup {
  id: string;
  title: string;
}

export interface Settings {
  path: string;
  appearance: Appearance;
  groups: KeyGroup[];
  keybinds: KeyAction[];
  sequence_timeout_ms: number;
  problems: string[];
}

export const MAC = /mac/i.test(navigator.platform);

let current: Settings | null = null;
let actions = new Map<ActionId, KeyAction>();
const subscribers = new Set<() => void>();

function apply(next: Settings) {
  current = next;
  actions = new Map(next.keybinds.map((action) => [action.id, action]));
  showAppearance(next.appearance);
  for (const notify of subscribers) notify();
}

/** Reads the settings, and follows the file from then on. It listens before reading, so
 * no change is missed. */
export async function startSettings() {
  await listen<Settings>("settings-changed", (event) => apply(event.payload));
  apply(await invoke<Settings>("settings"));
}

/** The settings, kept current. Null only until the first read comes back. */
export function useSettings(): Settings | null {
  return useSyncExternalStore(
    (notify) => {
      subscribers.add(notify);
      return () => subscribers.delete(notify);
    },
    () => current,
  );
}

/** Rejects with a message written to be shown as is. */
export async function saveBindings(id: ActionId, bindings: Binding[]) {
  apply(await invoke<Settings>("set_keybind", { id, bindings }));
}

export async function resetBindings(id: ActionId) {
  apply(await invoke<Settings>("reset_keybind", { id }));
}

/** Rejects with a message written to be shown as is. */
export async function saveAppearance(id: string, value: AppearanceValue) {
  apply(await invoke<Settings>("set_appearance", { id, value }));
}

export async function resetAppearance(id: string) {
  apply(await invoke<Settings>("reset_appearance", { id }));
}

export const showsSteps = () => shownValue("transcript.steps") === "shown";

/** Rejects with a message written to be shown as is. */
export const toggleSteps = () =>
  saveAppearance("transcript.steps", showsSteps() ? "folded" : "shown");

export const vimOn = () => shownValue("vim.enabled") === true;

/** Rejects with a message written to be shown as is. */
export const toggleVim = () => saveAppearance("vim.enabled", !vimOn());

let triedTheme: string | null = null;

/** Tries out a value everywhere it shows, including the native title bar, without saving it.
 * Null goes back to the settings. */
export function tryAppearance(preview: AppearancePreview | null) {
  previewAppearance(preview);
  const theme = current && previewedTheme(current.appearance, systemIsDark(), preview);
  if (theme === triedTheme) return;
  triedTheme = theme;
  void invoke("try_window_theme", { theme });
}

export const loadInstalledFonts = () => invoke<InstalledFont[]>("installed_fonts");

/** How an action's first key is shown, or undefined when it has none. */
export function keysLabel(id: ActionId): string | undefined {
  const binding = actions.get(id)?.bindings[0];
  return binding && bindingLabel(binding);
}

export function bindingLabel(binding: Binding): string {
  return formatBinding(binding, MAC);
}

/** How actions that belong together are shown as one, leaving out any without a key, or
 * undefined when none has one. In a text field it's the keys that still work there. */
export function groupedKeysLabel(ids: ActionId[], typing: boolean): string | undefined {
  const bindings = ids.flatMap((id) =>
    (actions.get(id)?.bindings ?? [])
      .filter((binding) => !typing || worksWhileTyping(binding.key, id))
      .slice(0, 1),
  );
  return bindings.length > 0 ? formatKeys(bindings, MAC) : undefined;
}

/** A tooltip that names the key, when there is one. */
export function titled(text: string, id: ActionId): string {
  const keys = keysLabel(id);
  return keys ? `${text} (${keys})` : text;
}

// The contexts whose actions run while typing in a text field. Anywhere else, a key
// without a modifier in a text field is just typing.
const TEXT_CONTEXTS = new Set(["composer", "new_topic", "sheet", "palette", "mcp_form"]);

interface KeyEventLike extends KeyPress {
  repeat: boolean;
  isTrusted: boolean;
  defaultPrevented: boolean;
  target: EventTarget | null;
  preventDefault(): void;
}

export const isTextField = (target: EventTarget | null) =>
  target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement;

/** The keys pressed so far that a sequence could still use, the latest last. */
let pressed: KeyPress[] = [];
let pressedOn: EventTarget | null = null;
let pressedTimer: ReturnType<typeof setTimeout> | undefined;

function forgetPressed() {
  clearTimeout(pressedTimer);
  pressed = [];
}

function remember(event: KeyboardEvent) {
  // A sequence is pressed in one place. "g" in a text box, then moving out of it, isn't half of one.
  if (event.target !== pressedOn) forgetPressed();
  pressedOn = event.target;
  clearTimeout(pressedTimer);
  pressed = [...pressed.slice(-7), event];
  pressedTimer = setTimeout(forgetPressed, sequenceTimeout());
}

export function sequenceTimeout(): number {
  return current?.sequence_timeout_ms ?? 1000;
}

type Match = { binding: Binding; state: "full" | "prefix" };

const stateOf = (key: string, event: KeyPress) =>
  isSequence(key) ? sequenceState(key, pressed, MAC) : chordMatches(key, event, MAC) ? "full" : null;

/** Whether a key can fire from a text field at all. A key that types only does in the
 * contexts made for it, and never as part of a sequence, because the first key would have
 * to be held back from the text box to find out. */
function worksWhileTyping(key: string, id: ActionId): boolean {
  if (!key.split(" ").some(isBareChord)) return true;
  return !isSequence(key) && TEXT_CONTEXTS.has(id.split(".")[0]);
}

function bindingFor(event: KeyEventLike, id: ActionId): Match | undefined {
  // A focused button already answers to these, and that has to keep working for
  // people moving through buttons with the keyboard.
  const onButton =
    event.target instanceof HTMLButtonElement &&
    (event.key === "Enter" || event.key === " ") &&
    !event.metaKey &&
    !event.ctrlKey &&
    !event.altKey;
  if (onButton) return undefined;
  const typing = isTextField(event.target);
  const terminal = inTerminal(event.target);
  let waiting: Match | undefined;
  for (const binding of actions.get(id)?.bindings ?? []) {
    if (typing && !worksWhileTyping(binding.key, id)) continue;
    // Shells and editors use every other key, Ctrl and Esc included.
    if (terminal && !usesCommand(binding.key)) continue;
    const state = stateOf(binding.key, event);
    if (state === "full") return { binding, state };
    if (state === "prefix") waiting ??= { binding, state };
  }
  return waiting;
}

let recording = false;

/** While a shortcut is being recorded, no key does what it normally does. */
export function pauseKeys(on: boolean) {
  recording = on;
  forgetPressed();
}

let holdTimer: ReturnType<typeof setTimeout> | undefined;

function cancelHold() {
  clearTimeout(holdTimer);
  holdTimer = undefined;
}

// macOS doesn't report a key coming up while Command is down, so a held Command shortcut
// is only cancelled early by letting go of Command.
window.addEventListener("keyup", cancelHold, true);
window.addEventListener("blur", () => {
  cancelHold();
  forgetPressed();
});
// This runs before every other listener, so a press is remembered by the time they ask.
window.addEventListener(
  "keydown",
  (event) => {
    // WebKit hands any key the page didn't prevent back to AppKit, where Esc ends full screen.
    // Dialogs and lists still close on it: neither they nor Base UI check defaultPrevented for Esc.
    if (event.key === "Escape") event.preventDefault();
    if (event.repeat || !event.isTrusted) return;
    cancelHold();
    if (chordOf(event, MAC)) remember(event);
  },
  true,
);

/** The press that has run an action. Each press runs one, which is how a global key wins
 * over a local one, and how pointing at a sidebar row takes A from a permission request. */
let claimed: Event | null = null;

const nativeOf = (event: KeyEventLike) =>
  ("nativeEvent" in event ? event.nativeEvent : event) as Event;

/** Runs the action if this press completes one of its keys, and says whether it ran. A key
 * that only starts a sequence is held back and returns false, so that other actions still
 * get to see it: the second G of gg also starts gt, and has to reach gg regardless.
 * Keys sent by `remapNative` aren't people pressing them, so they pass straight through. */
export function act(event: KeyEventLike, id: ActionId, run: () => void): boolean {
  if (recording || !event.isTrusted || claimed === nativeOf(event)) return false;
  const match = bindingFor(event, id);
  if (!match) return false;
  event.preventDefault();
  if (match.state === "prefix") return false;
  claimed = nativeOf(event);
  forgetPressed();
  const { binding } = match;
  if (binding.hold_ms > 0) {
    if (!event.repeat) holdTimer = setTimeout(run, binding.hold_ms);
  } else if (!event.repeat || binding.repeat) {
    run();
  }
  return true;
}

/** Whether an action has already run for this press. */
export const isClaimed = (event: KeyEventLike) => claimed === nativeOf(event);

/** Takes a press for something that isn't an action, like vim in a text box, so no action
 * runs for it after. */
export function claim(event: KeyEventLike) {
  claimed = nativeOf(event);
  forgetPressed();
}

/** Whether a press completes one of an action's keys, for screens that run it themselves. */
export function matches(event: KeyEventLike, id: ActionId): boolean {
  return actions.get(id)?.bindings.some((binding) => stateOf(binding.key, event) === "full") ?? false;
}

export function recordedChord(event: KeyPress): string | null {
  return chordOf(event, MAC);
}

/** Puts a Base UI list or dialog under the settings. It understands only its own keys, so
 * a bound key is sent on as the one it expects, and its own key does nothing if it's no
 * longer bound. Use it while capturing, so nothing inside sees the original press. */
export function remapNative(event: ReactKeyboardEvent, map: [ActionId, string][]) {
  if (recording || !event.isTrusted) return;
  const plain = !event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey;
  const target = event.target;
  for (const [id, native] of map) {
    const match = bindingFor(event, id);
    if (!match) continue;
    const already = match.state === "full" && plain && event.key === native;
    const { hold_ms, repeat } = match.binding;
    if (already && hold_ms === 0 && (repeat || !event.repeat)) return;
    event.stopPropagation();
    act(event, id, () => {
      target.dispatchEvent(new KeyboardEvent("keydown", { key: native, bubbles: true, cancelable: true }));
    });
    return;
  }
  if (plain && map.some(([, native]) => event.key === native)) {
    event.preventDefault();
    event.stopPropagation();
  }
}

/** The keys of a text box where Enter would otherwise add a line. */
export function textBoxKey(
  event: ReactKeyboardEvent,
  send: [ActionId, () => void],
  newline: ActionId,
  back: [ActionId, () => void],
) {
  // Enter also confirms an IME composition, which must not send.
  if (event.nativeEvent.isComposing) return;
  if (act(event, send[0], send[1])) return;
  // This goes through the editing commands so that undo still works.
  if (act(event, newline, () => document.execCommand("insertText", false, "\n"))) return;
  if (act(event, back[0], back[1])) return;
  if (event.key === "Enter") event.preventDefault();
}
