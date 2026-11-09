import type { ReactNode } from "react";
import { type ActionId, useSettings } from "../keybinds";
import { Keys } from "./StatusIcon";

/** Every settings screen: the sidebar's place says whose settings these are and lists their
 * sections, and the panel shows the section. */
export function SettingsLayout({
  heading,
  detail,
  sections,
  section,
  active = false,
  closeKeys,
  onSelect,
  onClose,
  children,
}: {
  heading: string;
  /** Which one of them, when there can be several. */
  detail?: string;
  sections: readonly { id: string; label: string }[];
  section: string;
  /** The list of sections has the keyboard. */
  active?: boolean;
  /** What closes the settings right now, if a key does. */
  closeKeys?: ActionId;
  onSelect?: (section: string) => void;
  onClose: () => void;
  children: ReactNode;
}) {
  return (
    <>
      <aside className={`settings-side ${active ? "active" : ""}`}>
        <div className="settings-side-head">
          <h1 className="settings-side-heading">{heading}</h1>
          {detail && (
            <span className="meta" title={detail}>
              {detail}
            </span>
          )}
        </div>
        <nav className="tree" aria-label="Settings sections">
          {sections.map(({ id, label }) => (
            <button
              key={id}
              className={`row ${id === section ? "current" : ""}`}
              tabIndex={-1}
              aria-current={id === section ? "page" : undefined}
              // Focus stays off these, so Enter does what the settings say rather than
              // pressing the button.
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => onSelect?.(id)}
            >
              <span className="name">{label}</span>
            </button>
          ))}
        </nav>
        <button className="row side-settings" tabIndex={-1} onClick={onClose}>
          <span className="name">Close settings</span>
          {closeKeys && <Keys action={closeKeys} />}
        </button>
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
