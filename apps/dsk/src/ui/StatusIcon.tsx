import done from "../../../../design/icons/done.svg?raw";
import failed from "../../../../design/icons/failed.svg?raw";
import idle from "../../../../design/icons/idle.svg?raw";
import stale from "../../../../design/icons/stale.svg?raw";
import waiting from "../../../../design/icons/waiting.svg?raw";
import working from "../../../../design/icons/working.svg?raw";
import { type ActionId, keysLabel } from "../keybinds";
import { STATE_LABEL, type StatusState } from "../model";

const ICONS: Record<StatusState, string> = { done, failed, idle, stale, waiting, working };

export function StatusIcon({ state }: { state: StatusState }) {
  return (
    <span
      className={`status-icon status-${state}`}
      role="img"
      aria-label={STATE_LABEL[state]}
      // Static files from design/icons, never anything from an agent.
      dangerouslySetInnerHTML={{ __html: ICONS[state] }}
    />
  );
}

export function Kbd({ children }: { children: string }) {
  return <kbd className="kbd">{children}</kbd>;
}

/** An action's key as it's currently set, and nothing when it has none. */
export function Keys({ action }: { action: ActionId }) {
  const label = keysLabel(action);
  return label ? <Kbd>{label}</Kbd> : null;
}

export function StopIcon() {
  return (
    <svg viewBox="0 0 12 12" fill="currentColor" aria-hidden="true">
      <rect width="12" height="12" rx="2" />
    </svg>
  );
}

export function SettingsIcon() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" />
      <circle cx="12" cy="12" r="3" />
    </svg>
  );
}
