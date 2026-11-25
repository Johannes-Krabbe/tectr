// TEMPORARY, dev builds only: records where the keyboard goes, to find why it sometimes ends up
// nowhere. Read it in the console with `__focusTrace()`. Remove once that's found.

interface TraceEntry {
  at: string;
  what: string;
  target: string;
  active: string;
  pageFocused: boolean;
}

const LIMIT = 300;
const entries: TraceEntry[] = [];

function describe(node: EventTarget | null): string {
  if (!(node instanceof Element)) return node === window ? "window" : String(node);
  const classes = node.classList.length > 0 ? `.${[...node.classList].join(".")}` : "";
  const connected = node.isConnected ? "" : " (removed)";
  const shown = node instanceof HTMLElement && node.getClientRects().length === 0 ? " (hidden)" : "";
  return `${node.tagName.toLowerCase()}${classes}${connected}${shown}`;
}

function record(what: string, target: EventTarget | null) {
  entries.push({
    at: new Date().toISOString().slice(11, 23),
    what,
    target: describe(target),
    active: describe(document.activeElement),
    pageFocused: document.hasFocus(),
  });
  if (entries.length > LIMIT) entries.shift();
}

export function traceFocus() {
  document.addEventListener("focusin", (event) => record("focusin", event.target), true);
  document.addEventListener(
    "focusout",
    (event) => record(`focusout -> ${describe(event.relatedTarget)}`, event.target),
    true,
  );
  window.addEventListener("focus", () => record("window focus", window));
  window.addEventListener("blur", () => record("window blur", window));
  document.addEventListener("visibilitychange", () => record(`visibility ${document.visibilityState}`, document));
  // First and last of every listener, so the trace says whether anything acted on the press.
  window.addEventListener("keydown", (event) => record(`keydown ${event.key}`, event.target), true);
  window.addEventListener("keydown", (event) =>
    record(`keydown ${event.key} done${event.defaultPrevented ? ", taken" : ""}`, event.target),
  );
  Object.assign(window, { __focusTrace: () => console.table(entries.slice(-60)) });
}
