import { ContextMenu } from "@base-ui/react/context-menu";
import { Menu } from "@base-ui/react/menu";
import { Fragment, type ReactElement, type ReactNode, type RefObject, useRef, useState } from "react";
import { type ActionId, remapNative } from "../keybinds";
import { Icon } from "./Icon";
import { PICKER_KEYS } from "./Picker";
import { Kbd } from "./StatusIcon";

export interface MenuItem {
  id: string;
  label: string;
  /** The key that does the same, so a menu also teaches it. */
  keys?: string;
  /** What's chosen next, for an item like "Move to" with a few places to go. It opens beside
   * the menu rather than running, and `run` is never called. */
  items?: MenuItem[];
  run: () => void;
}

/** Groups of items, with a line between groups. */
export type MenuGroups = MenuItem[][];

/** A picker's keys, plus going into a submenu and back out of it. */
const MENU_KEYS: [ActionId, string][] = [
  ...PICKER_KEYS,
  ["nav.right", "ArrowRight"],
  ["nav.left", "ArrowLeft"],
];

function Items({ group, ran }: { group: MenuItem[]; ran: RefObject<boolean> }) {
  return group.map((item) =>
    item.items ? (
      <Menu.SubmenuRoot key={item.id}>
        <Menu.SubmenuTrigger className="list-choice" disabled={item.items.length === 0}>
          <span className="name">{item.label}</span>
          <Icon name="unfold" />
        </Menu.SubmenuTrigger>
        <Menu.Portal>
          <Menu.Positioner side="right" align="start" sideOffset={4}>
            <List groups={[item.items]} ran={ran} />
          </Menu.Positioner>
        </Menu.Portal>
      </Menu.SubmenuRoot>
    ) : (
      <Menu.Item
        key={item.id}
        className="list-choice"
        onClick={() => {
          ran.current = true;
          item.run();
        }}
      >
        <span className="name">{item.label}</span>
        {item.keys && <Kbd>{item.keys}</Kbd>}
      </Menu.Item>
    ),
  );
}

function List({ groups, ran }: { groups: MenuGroups; ran: RefObject<boolean> }) {
  return (
    <Menu.Popup
      // A dropdown like any other, to `overlayOpen` in keys.ts as well as to the eye.
      className="select-popup"
      // Dismissing it puts focus back. An item decides where focus goes itself, and putting
      // it back would take it from the sheet that item just opened.
      finalFocus={() => !ran.current}
      onKeyDownCapture={(event) => remapNative(event, MENU_KEYS)}
    >
      {groups.map((group, index) => (
        <Fragment key={group[0]?.id ?? index}>
          {index > 0 && <Menu.Separator className="menu-separator" />}
          <Items group={group} ran={ran} />
        </Fragment>
      ))}
    </Menu.Popup>
  );
}

/** A button that opens a menu, which is where what's done now and then lives. It's in the Tab
 * order like any button, and follows the same keys as a picker once it's open. */
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
      >
        {children}
      </Menu.Trigger>
      <Menu.Portal>
        <Menu.Positioner align="end" sideOffset={4}>
          <List groups={groups} ran={ran} />
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
          {groups && <List groups={groups} ran={ran} />}
        </ContextMenu.Positioner>
      </ContextMenu.Portal>
    </ContextMenu.Root>
  );
}
