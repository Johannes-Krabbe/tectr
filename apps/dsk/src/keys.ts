// Who has the keyboard, and moving it between the sidebar and the main pane.

/** macOS webviews leave buttons out of Tab unless the person turned on keyboard navigation in
 * System Settings, but not when a button says where it goes in the order itself. So every
 * button that doesn't say gets a place, and Tab reaches every control, as it does in a native
 * app. A button that should be skipped, like one that only shows on hover, says -1. */
export function keepButtonsInTabOrder(root: HTMLElement) {
  const SELECTOR = "button:not([tabindex])";
  const place = (element: Element) => {
    if (element.matches(SELECTOR)) element.setAttribute("tabindex", "0");
    for (const button of element.querySelectorAll(SELECTOR)) button.setAttribute("tabindex", "0");
  };
  place(root);
  new MutationObserver((records) => {
    for (const record of records) {
      for (const node of record.addedNodes) if (node instanceof Element) place(node);
    }
  }).observe(root, { childList: true, subtree: true });
}

/** A sheet, dropdown or full-size image is open, and takes the keyboard from everything
 * behind it. */
export function overlayOpen(): boolean {
  return document.querySelector(".sheet, .select-popup, .viewer") !== null;
}

/** Where the keyboard was last, outside dropdowns, menus and sheets, which go away. */
let lastFocused: HTMLElement | null = null;
/** The row it was in, and where that row was in its page, for when the row itself goes. */
let lastRow: { page: HTMLElement; index: number } | null = null;

const OVERLAYS = ".sheet, .select-popup, .viewer";

const isShown = (element: HTMLElement) => element.isConnected && element.getClientRects().length > 0;

/** Puts the keyboard back where it was, or as close as there still is. */
function recoverFocus() {
  if (lastFocused && isShown(lastFocused)) {
    lastFocused.focus();
    return;
  }
  // The row went, like an Add row once its server is added: the row that took its place, as
  // a native list does.
  if (lastRow?.page.isConnected) {
    const rows = [...lastRow.page.querySelectorAll<HTMLElement>("[data-setting]")];
    const row = rows[Math.min(lastRow.index, rows.length - 1)];
    if (row) {
      row.focus();
      return;
    }
  }
  focusPane();
}

/** The keyboard never ends up on nothing. When what had it goes away, or a dropdown or menu
 * closes without handing it on, keys would reach no screen at all, and only a click would get
 * them back. So whenever focus is left on the page itself, it goes back to where it was, or as
 * close as there still is, the way a native app keeps a focused control. */
export function keepKeyboardSomewhere() {
  document.addEventListener("focusin", (event) => {
    const target = event.target;
    if (!(target instanceof HTMLElement) || target.closest(OVERLAYS)) return;
    lastFocused = target;
    const row = target.closest<HTMLElement>("[data-setting]");
    const page = row?.closest<HTMLElement>(".pane");
    lastRow = row && page ? { page, index: [...page.querySelectorAll("[data-setting]")].indexOf(row) } : null;
  });
  const lost = () => document.hasFocus() && document.activeElement === document.body && !overlayOpen();
  // Something that had focus going away doesn't always say so, and a dropdown hands focus on a
  // moment after it closes, so this looks once things have settled.
  document.addEventListener("focusout", () => {
    requestAnimationFrame(() => requestAnimationFrame(() => lost() && recoverFocus()));
  });
  // And a key pressed with nothing focused, from a removal no event told about, finds its way.
  window.addEventListener(
    "keydown",
    (event) => {
      if (event.target === document.body && lost()) recoverFocus();
    },
    true,
  );
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

export function subscribeFocus(notify: () => void) {
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
 * a terminal on its shell, a diff on its files and a workspace on its page. */
export function focusPane() {
  requestAnimationFrame(() => {
    const target = document.querySelector<HTMLElement>(
      ".create textarea, .import-search, .new-tab-kind:not(:disabled), .log, .terminal-host, .diff-body, .workspace-page",
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
