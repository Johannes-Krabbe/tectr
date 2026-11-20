import type { Device } from "./engine";
import { connectionProblem, isOnline } from "./model";
import { isLoopback } from "./pathInput";
import { Icon } from "./ui/Icon";
import { MenuButton } from "./ui/Menu";
import { SettingRow, SettingsGroupBox, SettingsPage } from "./ui/SettingsPage";

function status(device: Device): string {
  switch (device.connection.state) {
    case "connecting":
      return "Connecting";
    case "online":
      return device.connection.stalled ? "Not responding" : "Online";
    case "offline":
      return "Offline";
    case "asleep":
      return "Asleep";
    case "incompatible":
      return "Needs an update";
  }
}

/** Every device this app reaches: this machine's, and the ones paired from here. Each one's own
 * settings hold who can connect to it. */
export function DevicesSettings({
  devices,
  onAddDevice,
  onPair,
  onForget,
  onOpen,
}: {
  devices: Device[];
  onAddDevice: () => void;
  /** Undefined while this machine's daemon isn't reachable, which is where pairing starts. */
  onPair: (() => void) | undefined;
  onForget: (device: Device) => void;
  onOpen: (device: Device) => void;
}) {
  return (
    <SettingsPage>
      <SettingsGroupBox
        title="Devices"
        actions={
          <>
            <button className="button" disabled={!onPair} onClick={onPair}>
              Pair another machine
            </button>
            <button className="button primary" onClick={onAddDevice}>
              Add a device
            </button>
          </>
        }
      >
        {devices.map((device) => {
          const name = device.state?.machine.name ?? device.url;
          const local = isLoopback(device.url);
          return (
            <SettingRow
              key={device.url}
              label={name}
              detail={connectionProblem(device)}
              meta={
                <span className="meta">
                  {local ? "This machine" : "Paired"}, {status(device)}
                </span>
              }
              quiet={
                !local && (
                  <MenuButton
                    label={`Actions for ${name}`}
                    className="icon-button"
                    groups={[[{ id: "forget", label: "Remove from this app", run: () => onForget(device) }]]}
                  >
                    <Icon name="more" />
                  </MenuButton>
                )
              }
            >
              <button
                className="button small"
                disabled={!isOnline(device) || device.state === null}
                onClick={() => onOpen(device)}
              >
                Settings
              </button>
            </SettingRow>
          );
        })}
      </SettingsGroupBox>
    </SettingsPage>
  );
}
