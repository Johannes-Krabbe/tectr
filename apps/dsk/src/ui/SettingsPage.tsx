import { type ReactNode, useEffect } from "react";
import { act, type ActionId, useSettings } from "../keybinds";
import { inPane } from "../keys";
import { entryIndex, type SettingsGroup, type SettingsView } from "../settingsNav";
import { hint, useHints } from "../shortcuts";
import { Keys } from "./StatusIcon";

/** The list of sections, and how to get around it. The app's settings and each device's are
 * separate screens that share it, so going from one to another is one step in the same list. */
export interface SettingsNav {
  groups: SettingsGroup[];
  onGo: (view: SettingsView) => void;
  onClose: () => void;
}

const SECTION_HINTS = [
  hint("Go in", "sections.confirm", "sections.open"),
  hint("Section", "sections.up", "sections.down"),
  hint("First, last", "sections.first", "sections.last"),
  hint("Close", "sections.close"),
];

/** Every settings screen: the sidebar's place lists the sections, grouped by whose they are,
 * and the panel shows the open one. Like the main sidebar, the list has the keyboard first and
 * shows what it lands on, and you go into a section to change things in it. */
export function SettingsLayout({
  nav,
  current,
  inside,
  onInside,
  children,
}: {
  nav: SettingsNav;
  current: SettingsView;
  /** The section has the keyboard, rather than the list of sections. */
  inside: boolean;
  onInside: (inside: boolean) => void;
  children: ReactNode;
}) {
  const entries = nav.groups.flatMap((group) => group.entries);
  const index = entryIndex(entries, current);

  useHints("pane", inside ? null : SECTION_HINTS);

  useEffect(() => {
    if (inside) return;
    const onKey = (event: KeyboardEvent) => {
      if (!inPane(event)) return;
      const go = (to: number) => () => {
        const next = entries[to];
        if (next) nav.onGo(next.view);
      };
      const commands: [ActionId, () => void][] = [
        ["sections.up", go(index - 1)],
        ["sections.down", go(index + 1)],
        ["sections.first", go(0)],
        ["sections.last", go(entries.length - 1)],
        ["sections.open", () => onInside(true)],
        ["sections.confirm", () => onInside(true)],
        ["sections.close", nav.onClose],
      ];
      commands.some(([id, run]) => act(event, id, run));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  // With a few devices the list is longer than the window.
  useEffect(() => {
    document.querySelector(".settings-side .row.current")?.scrollIntoView({ block: "nearest" });
  }, [index]);

  return (
    <>
      <aside className={`settings-side ${inside ? "" : "active"}`}>
        <h1 className="settings-side-heading">Settings</h1>
        <nav className="tree" aria-label="Settings sections">
          {nav.groups.map((group) => (
            <section key={group.id} aria-label={group.heading}>
              <h2 className="side-group" title={group.heading}>
                {group.heading}
              </h2>
              {group.entries.map((entry) => {
                const shown = entries.indexOf(entry) === index;
                return (
                  <button
                    key={entry.view.section}
                    className={`row ${shown ? "current" : ""}`}
                    tabIndex={-1}
                    aria-current={shown ? "page" : undefined}
                    // Focus stays off these, so Enter does what the settings say rather than
                    // pressing the button.
                    onMouseDown={(event) => event.preventDefault()}
                    onClick={() => {
                      onInside(false);
                      nav.onGo(entry.view);
                    }}
                  >
                    <span className="name">{entry.label}</span>
                  </button>
                );
              })}
            </section>
          ))}
        </nav>
        <div className="side-foot">
          <button className="row" tabIndex={-1} onClick={nav.onClose}>
            <span className="name">Close settings</span>
            {!inside && <Keys action="sections.close" />}
          </button>
        </div>
      </aside>
      <main className="main">{children}</main>
    </>
  );
}

/** One section's page: the problems with the file come first, wherever you are, because
 * they affect all of them. */
export function SettingsPage({
  error,
  footer,
  children,
}: {
  error?: string | null;
  footer?: ReactNode;
  children: ReactNode;
}) {
  const settings = useSettings();
  return (
    <>
      <div className="pane settings">
        <div className="settings-in">
          {error && <p className="error settings-problem">{error}</p>}
          {settings?.problems.map((problem) => (
            <p key={problem} className="error settings-problem">
              {problem}
            </p>
          ))}
          {children}
        </div>
      </div>
      {footer && <div className="settings-actions">{footer}</div>}
    </>
  );
}

/** What a section's bar holds while the list of sections has the keyboard: the key that goes
 * into the section. */
export function GoInHint() {
  return (
    <span className="settings-actions-what">
      <Keys action="sections.confirm" /> Edit
    </span>
  );
}

/** Half of however many rows fit, like Ctrl+D and Ctrl+U move half a screen in Vim. */
export function halfScreen(): number {
  const height = document.querySelector(".pane.settings")?.clientHeight ?? 0;
  const row = document.querySelector<HTMLElement>(".keybind")?.offsetHeight || 1;
  return Math.max(Math.floor(height / row / 2), 1);
}

export const centerCurrentRow = () =>
  document.querySelector(".keybind.current")?.scrollIntoView({ block: "center" });

/** The keys every settings list moves with, and leaves by. */
export function listMoves(
  selected: number,
  count: number,
  setCursor: (index: number) => void,
  onLeave: () => void,
): [ActionId, () => void][] {
  const moveTo = (index: number) => () => setCursor(Math.max(Math.min(index, count - 1), 0));
  return [
    ["settings.up", moveTo(selected - 1)],
    ["settings.down", moveTo(selected + 1)],
    ["settings.half_up", () => moveTo(selected - halfScreen())()],
    ["settings.half_down", () => moveTo(selected + halfScreen())()],
    ["settings.first", moveTo(0)],
    ["settings.last", moveTo(count - 1)],
    ["settings.center", centerCurrentRow],
    ["settings.back", onLeave],
    ["settings.parent", onLeave],
  ];
}
