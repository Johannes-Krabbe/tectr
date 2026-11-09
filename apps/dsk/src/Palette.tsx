import { Autocomplete } from "@base-ui/react/autocomplete";
import { Dialog } from "@base-ui/react/dialog";
import { useRef } from "react";
import { type ActionId, remapNative } from "./keybinds";
import type { StatusState } from "./model";
import { hint, useHints } from "./shortcuts";
import { Kbd, StatusIcon } from "./ui/StatusIcon";

export interface PaletteItem {
  id: string;
  label: string;
  detail?: string;
  keys?: string;
  state?: StatusState;
  run: () => void;
}

const KEYS: [ActionId, string][] = [
  ["palette.up", "ArrowUp"],
  ["palette.down", "ArrowDown"],
  ["palette.confirm", "Enter"],
  ["palette.cancel", "Escape"],
];

const HINTS = [
  hint("Move", "palette.up", "palette.down"),
  hint("Run", "palette.confirm"),
  hint("Close", "palette.cancel"),
];

/** Every word typed has to appear somewhere in the item. */
function matches(item: PaletteItem, query: string): boolean {
  const text = `${item.label} ${item.detail ?? ""}`.toLowerCase();
  return query
    .toLowerCase()
    .split(/\s+/)
    .every((word) => text.includes(word));
}

/** Reaches every action and topic, and shows each one's shortcut. A list to choose from, like
 * archived topics, says what it holds with `placeholder` and what having none means with
 * `empty`. */
export function Palette({
  items,
  onClose,
  placeholder = "Find a topic or command",
  empty = "Nothing matches",
}: {
  items: PaletteItem[];
  onClose: () => void;
  placeholder?: string;
  empty?: string;
}) {
  // Dismissing it puts focus back. A command decides where focus goes itself, and
  // putting it back would take it from the sheet that command just opened.
  const ran = useRef(false);
  useHints("overlay", HINTS);
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onClose()}>
      <Dialog.Portal>
        <Dialog.Backdrop className="scrim" />
        <Dialog.Popup
          className="sheet"
          aria-label={placeholder}
          finalFocus={() => !ran.current}
          onKeyDownCapture={(event) => remapNative(event, KEYS)}
        >
          <Autocomplete.Root open inline items={items} autoHighlight="always" keepHighlight filter={matches}>
            <Autocomplete.Input
              className="sheet-name"
              placeholder={placeholder}
              aria-label={placeholder}
              spellCheck={false}
              autoCorrect="off"
              autoCapitalize="off"
            />
            <Autocomplete.Empty>
              <p className="palette-empty">{items.length ? "Nothing matches" : empty}</p>
            </Autocomplete.Empty>
            <Autocomplete.List className="palette-list">
              {(item: PaletteItem) => (
                <Autocomplete.Item
                  key={item.id}
                  value={item}
                  className="palette-item"
                  onClick={() => {
                    ran.current = true;
                    onClose();
                    item.run();
                  }}
                >
                  {item.state && <StatusIcon state={item.state} />}
                  <span className="name">{item.label}</span>
                  {item.detail && <span className="meta">{item.detail}</span>}
                  {item.keys && <Kbd>{item.keys}</Kbd>}
                </Autocomplete.Item>
              )}
            </Autocomplete.List>
          </Autocomplete.Root>
        </Dialog.Popup>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
