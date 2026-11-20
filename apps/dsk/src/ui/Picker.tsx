import { Select } from "@base-ui/react/select";
import type { ReactNode, RefObject } from "react";
import { type ActionId, remapNative } from "../keybinds";

/** The keys of any list that drops down, which the menus follow too. */
export const PICKER_KEYS: [ActionId, string][] = [
  ["picker.up", "ArrowUp"],
  ["picker.down", "ArrowDown"],
  ["picker.first", "Home"],
  ["picker.last", "End"],
  ["picker.confirm", "Enter"],
  ["picker.cancel", "Escape"],
];

/** A dropdown that follows the settings' keys. When it closes, focus goes back to its button,
 * so the keyboard carries on from where it was, unless `finalFocus` names somewhere else. */
export function Picker({
  label,
  className,
  value,
  disabled,
  open,
  onOpenChange,
  onChange,
  finalFocus,
  title,
  trigger,
  children,
}: {
  label: string;
  className: string;
  value: string | null;
  disabled?: boolean;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onChange: (value: string) => void;
  finalFocus?: RefObject<HTMLElement | null>;
  title?: string;
  trigger: ReactNode;
  children: ReactNode;
}) {
  return (
    <Select.Root
      value={value}
      disabled={disabled}
      open={open}
      onOpenChange={onOpenChange}
      onValueChange={(next) => {
        if (next !== null) onChange(next);
      }}
    >
      <Select.Trigger className={className} aria-label={label} title={title}>
        {trigger}
      </Select.Trigger>
      <Select.Portal>
        <Select.Positioner alignItemWithTrigger={false} align="start" sideOffset={4}>
          <Select.Popup
            className="select-popup"
            finalFocus={finalFocus}
            onKeyDownCapture={(event) => remapNative(event, PICKER_KEYS)}
          >
            <Select.List>{children}</Select.List>
          </Select.Popup>
        </Select.Positioner>
      </Select.Portal>
    </Select.Root>
  );
}

export function Choice({
  value,
  label,
  meta,
  disabled,
  leading,
  onHighlight,
}: {
  value: string;
  label: string;
  meta?: string;
  disabled?: boolean;
  leading?: ReactNode;
  /** The list moved here, by key or by pointer. It moves focus to do that. */
  onHighlight?: () => void;
}) {
  return (
    <Select.Item
      className="list-choice"
      value={value}
      disabled={disabled}
      onFocus={onHighlight}
    >
      {leading}
      <Select.ItemText className="name">{label}</Select.ItemText>
      {meta && <span className="meta">{meta}</span>}
    </Select.Item>
  );
}
