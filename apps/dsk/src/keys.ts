// Who has the keyboard, and moving it between the sidebar and the main pane.
//
// macOS webviews skip buttons and radios on Tab unless the person turned on keyboard
// navigation in System Settings. So every control needs its own key, and focus
// moves here rather than relying on Tab.

import type { KeyboardPlace } from "./shortcuts";

/** A sheet, dropdown or full-size image is open, and takes the keyboard from everything
 * behind it. */
export function overlayOpen(): boolean {
  return document.querySelector(".sheet, .select-popup, .viewer") !== null;
}

/** A sheet is open. The palette is one too, and so is a full-size image. */
export function sheetOpen(): boolean {
  return document.querySelector(".sheet, .viewer") !== null;
}

/** Whether a key is typed into a terminal, where only Cmd shortcuts are the app's. */
export function inTerminal(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest(".terminal") !== null;
}

/** A key meant for the main pane. The sidebar handles its own. */
export function inPane(event: KeyboardEvent): boolean {
  const { target } = event;
  return !overlayOpen() && !(target instanceof Element && target.closest(".side"));
}

/** What has the keyboard, down to which kind of row in the sidebar. */
export function keyboardPlace(): KeyboardPlace {
  const active = document.activeElement;
  if (active instanceof HTMLInputElement || active instanceof HTMLTextAreaElement) return "typing";
  if (!active?.closest(".side")) return "pane";
  const row = active.closest(".row");
  if (row?.matches(".device")) return "sidebar-device";
  if (row?.matches(".workspace")) return "sidebar-workspace";
  if (row?.matches(".topic")) return "sidebar-topic";
  if (row?.matches(".empty")) return "sidebar-empty";
  return "sidebar";
}

export function subscribeKeyboardPlace(notify: () => void) {
  document.addEventListener("focusin", notify);
  document.addEventListener("focusout", notify);
  return () => {
    document.removeEventListener("focusin", notify);
    document.removeEventListener("focusout", notify);
  };
}

/** Gives the main pane the keyboard once the current update has rendered. It lands on
 * the transcript rather than the composer, so single keys work straight away. Starting an
 * agent has no transcript, and exists to be typed into. A new tab lands on its first choice,
 * and a terminal on its shell. */
export function focusPane() {
  requestAnimationFrame(() => {
    const target = document.querySelector<HTMLElement>(
      ".create textarea, .new-tab-kind:not(:disabled), .log, .terminal-host",
    );
    if (target) target.focus();
    else if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
  });
}

export function inSidebar(): boolean {
  return document.activeElement?.closest(".side") != null;
}

/** Lands on the open topic's row, or the first row when no topic is open. */
export function focusSidebar() {
  const row =
    document.querySelector<HTMLElement>('.side [data-nav][tabindex="0"]') ??
    document.querySelector<HTMLElement>(".side [data-nav]");
  row?.focus();
}

/** The sidebar row that adding goes into when the pointer is on it. */
export function pointedRow(): HTMLElement | null {
  return document.querySelector<HTMLElement>(".side .tree .row:hover");
}

/** Presses the + that a row adds through: its own, or its workspace's for a topic. */
export function pressAdd(row: HTMLElement) {
  const add = row.matches(".empty")
    ? row
    : (row.querySelector<HTMLElement>(".add") ??
      row.parentElement?.querySelector<HTMLElement>(".row.workspace .add"));
  add?.click();
}
