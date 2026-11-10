import { ContextMenu } from "@base-ui/react/context-menu";
import { Menu } from "@base-ui/react/menu";
import { Fragment, type ReactElement, type ReactNode, type RefObject, useRef, useState } from "react";
import { remapNative } from "../keybinds";
import { useHints } from "../shortcuts";
import { PICKER_HINTS, PICKER_KEYS } from "./Picker";
import { Kbd } from "./StatusIcon";

export interface MenuItem {
  id: string;
  label: string;
  /** The key that does the same, so a menu also teaches it. */
  keys?: string;
  run: () => void;
}

/** Groups of items, with a line between groups. */
export type MenuGroups = MenuItem[][];

/** Only rendered while its menu is open, which is what makes the hints follow it. */
function Popup({ groups, ran }: { groups: MenuGroups; ran: RefObject<boolean> }) {
  useHints("overlay", PICKER_HINTS);
  return (
    <Menu.Popup
      // A dropdown like any other, to `overlayOpen` in keys.ts as well as to the eye.
      className="select-popup"
      // Dismissing it puts focus back. An item decides where focus goes itself, and putting
      // it back would take it from the sheet that item just opened.
      finalFocus={() => !ran.current}
      onKeyDownCapture={(event) => remapNative(event, PICKER_KEYS)}
    >
      {groups.map((group, index) => (
        <Fragment key={group[0]?.id ?? index}>
          {index > 0 && <Menu.Separator className="menu-separator" />}
          {group.map((item) => (
            <Menu.Item
              key={item.id}
              className="palette-item"
              onClick={() => {
                ran.current = true;
                item.run();
              }}
            >
              <span className="name">{item.label}</span>
              {item.keys && <Kbd>{item.keys}</Kbd>}
            </Menu.Item>
          ))}
        </Fragment>
      ))}
    </Menu.Popup>
  );
}

/** A button that opens a menu of things the palette also has, for reaching them by pointer.
 * It follows the same keys as a picker once it's open. */
export function MenuButton({
  label,
  className,
  groups,
  disabled,
  title = label,
  children,
}: {
  label: string;
  className: string;
  groups: MenuGroups;
  disabled?: boolean;
  title?: string;
  children: ReactNode;
}) {
  const ran = useRef(false);
  return (
    <Menu.Root
      onOpenChange={(open) => {
        if (open) ran.current = false;
      }}
    >
      <Menu.Trigger
        className={className}
        aria-label={label}
        title={title}
        disabled={disabled || groups.length === 0}
        tabIndex={-1}
      >
        {children}
      </Menu.Trigger>
      <Menu.Portal>
        <Menu.Positioner align="end" sideOffset={4}>
          <Popup groups={groups} ran={ran} />
        </Menu.Positioner>
      </Menu.Portal>
    </Menu.Root>
  );
}

/** One menu for a whole area, opened where the pointer is by a right click. `groupsAt` says
 * what the thing under the pointer offers, and where that's nothing, nothing opens. One for
 * the area rather than one per row keeps a long list cheap. */
export function RightClickMenu({
  groupsAt,
  render,
  children,
}: {
  groupsAt: (target: Element) => MenuGroups;
  /** The element the area is. */
  render: ReactElement;
  children: ReactNode;
}) {
  const [groups, setGroups] = useState<MenuGroups | null>(null);
  // What the click landed on, found before the menu is told to open.
  const pointed = useRef<MenuGroups>([]);
  const ran = useRef(false);
  return (
    <ContextMenu.Root
      open={groups !== null}
      onOpenChange={(open) => {
        if (open) ran.current = false;
        setGroups(open && pointed.current.length > 0 ? pointed.current : null);
      }}
    >
      <ContextMenu.Trigger
        render={render}
        onContextMenuCapture={(event) => {
          pointed.current = event.target instanceof Element ? groupsAt(event.target) : [];
        }}
      >
        {children}
      </ContextMenu.Trigger>
      <ContextMenu.Portal>
        <ContextMenu.Positioner>
          {groups && <Popup groups={groups} ran={ran} />}
        </ContextMenu.Positioner>
      </ContextMenu.Portal>
    </ContextMenu.Root>
  );
}
