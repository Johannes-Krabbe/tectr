import {
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { act, isTextField, titled, useSettings } from "../keybinds";
import { overlayOpen } from "../keys";
import { entryIndex, type SettingsGroup, type SettingsView } from "../settingsNav";
import { Icon } from "./Icon";

/** The list of sections, and how to get around it. The app's settings and each device's are
 * separate screens that share it, so going from one to another is one step in the same list. */
export interface SettingsNav {
  groups: SettingsGroup[];
  onGo: (view: SettingsView) => void;
  onClose: () => void;
}

/** A list that's one stop for Tab, like a native list, whose items the page's row keys move
 * through. The item that last had focus is the one Tab comes back to. */
export function useRovingList(count: number) {
  const [at, setAt] = useState(0);
  const current = Math.max(Math.min(at, count - 1), 0);
  return {
    current,
    /** Spread on each item, in order. */
    item: (index: number) => ({
      tabIndex: index === current ? 0 : -1,
      "data-roving": "",
      onFocus: () => setAt(index),
    }),
  };
}

/** A press on something drawn inside this element. Dropdowns and sheets are drawn elsewhere on
 * the page, but their keys still come up through React's tree, and they're theirs. */
const pressedInside = (event: ReactKeyboardEvent<HTMLElement>) =>
  event.target instanceof Node && event.currentTarget.contains(event.target);

/** The rows of the page that's showing, in order. */
const pageRows = () => [...document.querySelectorAll<HTMLElement>(".settings [data-setting]")];

/** Gives the page the keyboard, on its first row. */
const focusFirstRow = () => pageRows()[0]?.focus();

/** The control Enter presses for a row: the first that isn't its reset button or one of the
 * controls that only show on hover. A text field takes the keyboard instead. */
function activateRow(row: HTMLElement) {
  // A row that's a button itself, like a topic, is pressed.
  if (row instanceof HTMLButtonElement) {
    row.click();
    return;
  }
  const control = [
    ...row.querySelectorAll<HTMLElement>(
      ".setting-line input, .setting-line [role=switch], .setting-line [role=combobox], .setting-line [role=spinbutton], .setting-line button",
    ),
  ].find((element) => !element.closest(".reset, .setting-quiet") && !element.matches(":disabled"));
  // With nothing else to press, the row's menu has what it offers.
  const menu = row.querySelector<HTMLElement>(".setting-quiet [aria-haspopup]:not(:disabled)");
  if (!control) {
    menu?.click();
    return;
  }
  if (control instanceof HTMLInputElement || control.matches("[role=spinbutton]")) control.focus();
  else control.click();
}

/** Every settings screen: the sidebar's place lists the sections, grouped by whose they are,
 * and the panel shows the open one. The list is one Tab stop, and moving through it shows each
 * section, the way the main sidebar shows topics, and going right or Enter goes into the page.
 * Esc in a page comes back to the list, and Esc on the list closes settings. */
export function SettingsLayout({
  nav,
  current,
  children,
}: {
  nav: SettingsNav;
  current: SettingsView;
  children: ReactNode;
}) {
  const entries = nav.groups.flatMap((group) => group.entries);
  const index = entryIndex(entries, current);
  const list = useRef<HTMLElement>(null);
  // Moving by key shows the section and keeps the keyboard on its row once it has rendered.
  const focusCurrent = useRef(false);

  useLayoutEffect(() => {
    const row = list.current?.querySelector<HTMLElement>(".row.current");
    row?.scrollIntoView({ block: "nearest" });
    if (focusCurrent.current) row?.focus();
    focusCurrent.current = false;
  }, [index]);

  // Opening settings starts on the list, so the arrows work straight away.
  useEffect(() => {
    if (document.activeElement === document.body) {
      list.current?.querySelector<HTMLElement>(".row.current")?.focus();
    }
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (overlayOpen() || isTextField(event.target)) return;
      act(event, "sections.close", nav.onClose);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const onListKey = (event: ReactKeyboardEvent<HTMLElement>) => {
    const go = (to: number) => () => {
      const next = entries[Math.max(Math.min(to, entries.length - 1), 0)];
      if (!next) return;
      focusCurrent.current = true;
      nav.onGo(next.view);
    };
    act(event, "sections.up", go(index - 1)) ||
      act(event, "sections.down", go(index + 1)) ||
      act(event, "sections.first", go(0)) ||
      act(event, "sections.last", go(entries.length - 1)) ||
      act(event, "sections.open", focusFirstRow);
  };

  return (
    <>
      <aside className="settings-side">
        <h1 className="settings-side-heading">Settings</h1>
        <nav ref={list} className="tree" aria-label="Settings sections" onKeyDown={onListKey}>
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
                    tabIndex={shown ? 0 : -1}
                    aria-current={shown ? "page" : undefined}
                    // Enter on the section that's showing goes into it. A click only shows it.
                    onClick={(event) => {
                      nav.onGo(entry.view);
                      if (shown && event.detail === 0) focusFirstRow();
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
          <button className="row" title={titled("Close settings", "sections.close")} onClick={nav.onClose}>
            <span className="name">Close settings</span>
          </button>
        </div>
      </aside>
      <main className="main">{children}</main>
    </>
  );
}

/** One section's page: the problems with the file come first, wherever you are, because
 * they affect all of them. */
/** The keys of a page of rows, for the element that holds them: up and down go from row to row,
 * and `back`, when there is one, leaves the page. What a row does with the other keys is its own,
 * in `SettingRow`. Settings pages and a workspace's page both move this way. */
export function onRowKeys(event: ReactKeyboardEvent<HTMLElement>, back?: () => void) {
  if (!pressedInside(event)) return;
  const rows = [...event.currentTarget.querySelectorAll<HTMLElement>("[data-setting]")];
  const index = rows.findIndex((row) => row.contains(document.activeElement));
  const go = (to: number) => () => rows[Math.max(Math.min(to, rows.length - 1), 0)]?.focus();
  act(event, "settings.up", go(index === -1 ? 0 : index - 1)) ||
    act(event, "settings.down", go(index + 1)) ||
    act(event, "settings.first", go(0)) ||
    act(event, "settings.last", go(rows.length - 1)) ||
    (back !== undefined && act(event, "settings.back", back));
}

export function SettingsPage({ error, children }: { error?: string | null; children: ReactNode }) {
  const settings = useSettings();
  const toSections = () => document.querySelector<HTMLElement>(".settings-side .row.current")?.focus();
  return (
    <div className="pane settings" onKeyDown={(event) => onRowKeys(event, toSections)}>
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
  );
}

/** A titled run of rows, with what applies to all of them, like Add, beside the title. */
export function SettingsGroupBox({
  title,
  actions,
  children,
}: {
  title: string;
  actions?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="settings-group">
      <div className="settings-group-head">
        <h3 className="settings-group-title">{title}</h3>
        {actions}
      </div>
      <div className="settings-rows">{children}</div>
    </section>
  );
}

/** One row: what it is on the left, and what changes it on the right, where it can be used
 * straight away. `quiet` holds controls that only show while the row is pointed at or has focus,
 * like its menu, at the end of the row. They stay in the Tab order, and show once Tab reaches
 * them. With the keyboard on the row, left and right go through its values with `onStep`, and
 * Enter does what clicking its control does. */
export function SettingRow({
  label,
  detail,
  meta,
  nested,
  dim,
  quiet,
  error,
  onStep,
  children,
}: {
  label: ReactNode;
  /** A second line of what it is, in code type, such as the command a server runs. */
  detail?: ReactNode;
  meta?: ReactNode;
  /** Belongs to the row above it, like a server's profile. */
  nested?: boolean;
  /** Turned off, or otherwise not in use. */
  dim?: boolean;
  quiet?: ReactNode;
  /** What the last thing tried on this row couldn't do. */
  error?: string;
  /** The previous or next value, for a row whose value goes in steps or through choices. */
  onStep?: (direction: -1 | 1) => void;
  children?: ReactNode;
}) {
  return (
    <div
      className={`setting ${nested ? "nested" : ""} ${dim ? "dim" : ""}`}
      // Reached by the page's keys rather than by Tab, which goes through the controls.
      tabIndex={-1}
      data-setting=""
      onKeyDown={(event) => {
        if (!pressedInside(event)) return;
        const row = event.currentTarget;
        if (onStep && act(event, "settings.decrease", () => onStep(-1))) return;
        if (onStep && act(event, "settings.increase", () => onStep(1))) return;
        act(event, "settings.edit", () => activateRow(row));
      }}
    >
      <div className="setting-line">
        <span className="setting-label">
          <span className="name">{label}</span>
          {detail && <span className="setting-detail code">{detail}</span>}
        </span>
        {meta}
        {children}
        {quiet && <span className="setting-quiet">{quiet}</span>}
      </div>
      {error && <p className="setting-error">{error}</p>}
    </div>
  );
}

/** Shows that a setting differs from its default, and puts it back. */
export function ResetButton({ label, onReset }: { label: string; onReset: () => void }) {
  return (
    <button className="icon-button reset" title="Reset to the default" aria-label={`Reset ${label}`} onClick={onReset}>
      <Icon name="reset" />
    </button>
  );
}

/** A number that goes up and down in steps. It's one Tab stop, and the arrows step it like a
 * native stepper. The buttons are for the pointer. */
export function Stepper({
  label,
  value,
  shown,
  min,
  max,
  onStep,
}: {
  label: string;
  value: number;
  shown: string;
  min: number;
  max: number;
  onStep: (direction: -1 | 1) => void;
}) {
  return (
    <span
      className="stepper"
      role="spinbutton"
      tabIndex={0}
      aria-label={label}
      aria-valuenow={value}
      aria-valuemin={min}
      aria-valuemax={max}
      aria-valuetext={shown}
      onKeyDown={(event) => {
        const up = event.key === "ArrowUp" || event.key === "ArrowRight";
        const down = event.key === "ArrowDown" || event.key === "ArrowLeft";
        if (!up && !down) return;
        event.preventDefault();
        event.stopPropagation();
        onStep(up ? 1 : -1);
      }}
    >
      <button
        className="stepper-button"
        tabIndex={-1}
        aria-label={`Lower ${label}`}
        disabled={value <= min}
        onClick={() => onStep(-1)}
      >
        <Icon name="lower" />
      </button>
      <span className="stepper-value">{shown}</span>
      <button
        className="stepper-button"
        tabIndex={-1}
        aria-label={`Raise ${label}`}
        disabled={value >= max}
        onClick={() => onStep(1)}
      >
        <Icon name="raise" />
      </button>
    </span>
  );
}

/** On or off, pressed with a click, Space or Enter. */
export function Switch({
  label,
  checked,
  disabled,
  title,
  onChange,
}: {
  label: string;
  checked: boolean;
  disabled?: boolean;
  title?: string;
  onChange: (checked: boolean) => void;
}) {
  return (
    <button
      className="switch"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      title={title}
      disabled={disabled}
      onClick={() => onChange(!checked)}
    />
  );
}
