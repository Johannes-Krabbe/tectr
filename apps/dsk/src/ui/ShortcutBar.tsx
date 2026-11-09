import { useSyncExternalStore } from "react";
import { groupedKeysLabel, useSettings } from "../keybinds";
import { keyboardPlace, subscribeKeyboardPlace } from "../keys";
import { chooseHints, type Hint, useHintEntries } from "../shortcuts";
import { Kbd } from "./StatusIcon";

function Shortcut({ hint, typing }: { hint: Hint; typing: boolean }) {
  const keys = groupedKeysLabel(hint.actions, typing);
  if (!keys && hint.actions.length > 0) return null;
  return (
    <span className="shortcut">
      {keys && <Kbd>{keys}</Kbd>}
      <span className="shortcut-label">{hint.label}</span>
    </span>
  );
}

/** The line along the bottom of the window, with the keys that work right now. */
export function ShortcutBar() {
  // Rendering again when the settings change is what updates the keys shown.
  useSettings();
  const place = useSyncExternalStore(subscribeKeyboardPlace, keyboardPlace);
  const { hints, pinned } = chooseHints(useHintEntries(), place);
  const typing = place === "typing";
  return (
    <footer
      className="shortcut-bar"
      aria-label="Keyboard shortcuts"
      // Pressing on it mustn't take the keyboard, which would change what it shows.
      onMouseDown={(event) => event.preventDefault()}
    >
      <div className="shortcut-bar-hints">
        {hints.map((hint, index) => (
          <Shortcut key={`${index}/${hint.label}`} hint={hint} typing={typing} />
        ))}
      </div>
      {pinned.map((hint) => (
        <Shortcut key={hint.label} hint={hint} typing={typing} />
      ))}
    </footer>
  );
}
