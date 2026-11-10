import { useEffect, useState } from "react";
import { Row, keepFocus, useScrollToCurrent } from "./DeviceSettings";
import type { Device } from "./engine";
import { act, type ActionId } from "./keybinds";
import { inPane } from "./keys";
import { isOnline } from "./model";
import { isLoopback } from "./pathInput";
import { hint, SETTINGS_MOVES, useHints } from "./shortcuts";
import { GoInHint, listMoves, SettingsPage } from "./ui/SettingsPage";
import { Keys } from "./ui/StatusIcon";

function status(device: Device): string {
  switch (device.connection.state) {
    case "connecting":
      return "Connecting";
    case "online":
      return "Online";
    case "offline":
      return "Offline";
    case "incompatible":
      return "Needs an update";
  }
}

/** Every device this app reaches: this machine's, and the ones paired from here. Each one's own
 * settings hold who can connect to it. */
export function DevicesSettings({
  devices,
  active,
  onActivate,
  onLeave,
  onAddDevice,
  onPair,
  onForget,
  onOpen,
}: {
  devices: Device[];
  active: boolean;
  onActivate: () => void;
  onLeave: () => void;
  onAddDevice: () => void;
  /** Undefined while this machine's daemon isn't reachable, which is where pairing starts. */
  onPair: (() => void) | undefined;
  onForget: (device: Device) => void;
  onOpen: (device: Device) => void;
}) {
  const [cursor, setCursor] = useState(0);
  const selected = Math.max(Math.min(cursor, devices.length - 1), 0);
  const device: Device | undefined = devices[selected];
  const removable = device !== undefined && !isLoopback(device.url);
  const openable = device !== undefined && isOnline(device) && device.state !== null;

  useScrollToCurrent(selected, active);
  useHints(
    "pane",
    active
      ? [
          ...(openable ? [hint("Settings", "settings.edit")] : []),
          ...(removable ? [hint("Remove", "pairing.remove")] : []),
          hint("Add a device", "pairing.add_device"),
          ...(onPair ? [hint("Pair", "pairing.add")] : []),
          ...SETTINGS_MOVES,
        ]
      : null,
  );

  useEffect(() => {
    if (!active) return;
    const onKey = (event: KeyboardEvent) => {
      if (!inPane(event)) return;
      const commands: [ActionId, () => void][] = listMoves(selected, devices.length, setCursor, onLeave);
      if (device && openable) commands.push(["settings.edit", () => onOpen(device)]);
      if (device && removable) commands.push(["pairing.remove", () => onForget(device)]);
      commands.push(["pairing.add_device", onAddDevice]);
      if (onPair) commands.push(["pairing.add", onPair]);
      commands.some(([id, run]) => act(event, id, run));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const footer =
    active && device ? (
      <>
        <span className="settings-actions-what">
          {device.connection.state === "offline" ? device.connection.reason : device.url}
        </span>
        {removable && (
          <button className="button" onMouseDown={keepFocus} onClick={() => onForget(device)}>
            Remove
            <Keys action="pairing.remove" />
          </button>
        )}
        <button
          className="button"
          disabled={!openable}
          onMouseDown={keepFocus}
          onClick={() => onOpen(device)}
        >
          Settings
          <Keys action="settings.edit" />
        </button>
      </>
    ) : (
      <GoInHint />
    );

  return (
    <SettingsPage footer={footer}>
      <section className="settings-group">
        <div className="settings-group-head">
          <h3 className="settings-group-title">Devices</h3>
          <button className="button" disabled={!onPair} onMouseDown={keepFocus} onClick={onPair}>
            Pair another machine
            {active && <Keys action="pairing.add" />}
          </button>
          <button className="button primary" onMouseDown={keepFocus} onClick={onAddDevice}>
            Add a device
            {active && <Keys action="pairing.add_device" />}
          </button>
        </div>
        <div className="keybinds">
          {devices.map((d, index) => (
            <Row
              key={d.url}
              current={active && index === selected}
              onPoint={() => {
                setCursor(index);
                onActivate();
              }}
            >
              <span className="name">{d.state?.machine.name ?? d.url}</span>
              <span className="meta">
                {isLoopback(d.url) ? "This machine" : "Paired"}, {status(d)}
              </span>
            </Row>
          ))}
        </div>
      </section>
    </SettingsPage>
  );
}
