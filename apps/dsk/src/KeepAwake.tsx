import { useState } from "react";
import { type Device, errorText, request } from "./engine";
import { act } from "./keybinds";
import { awakeUntil, isOnline, KEEP_AWAKE_SUGGESTIONS, parseDuration, timeLeft } from "./model";
import { Icon } from "./ui/Icon";
import { Choice, Picker } from "./ui/Picker";
import { useNow } from "./ui/useNow";

const CUSTOM = "custom";
const STOP = "stop";

/** Keeps a device awake for a while: a dropdown of times, how long is left, and a box for a
 * time of one's own. Settings has it in a row, and the device's page next to its status. */
export function KeepAwakeFor({
  device,
  label = "Keep awake for a while",
  onError,
}: {
  device: Device;
  label?: string;
  /** `null` takes back the last one. */
  onError: (error: string | null) => void;
}) {
  const state = device.state;
  const online = isOnline(device);
  // Typing a time of one's own, rather than choosing one of the suggestions.
  const [customTime, setCustomTime] = useState<string | null>(null);
  const [picking, setPicking] = useState(false);
  const now = useNow(state?.awake_until_ms !== undefined);
  const awakeTill = awakeUntil(state, now);
  if (!state) return null;
  const deviceId = state.machine.id;

  const keepAwakeFor = (seconds: number | null) => {
    if (!online) return;
    onError(null);
    request(deviceId, { command: "keep_awake_for", seconds }).catch((reason) =>
      onError(errorText(reason)),
    );
  };

  const keepAwakeForTyped = (typed: string) => {
    const seconds = parseDuration(typed);
    if (seconds === null) {
      onError("Type a time such as 2h, 90m or 1h30m, up to 7 days");
      return;
    }
    setCustomTime(null);
    keepAwakeFor(seconds);
  };

  const chooseTime = (value: string) => {
    setPicking(false);
    if (value === CUSTOM) setCustomTime("");
    else if (value === STOP) keepAwakeFor(null);
    else keepAwakeForTyped(value);
  };

  if (customTime !== null) {
    return (
      <input
        className="settings-input short"
        autoFocus
        value={customTime}
        aria-label="How long to keep it awake"
        placeholder="2h, 90m, 1d"
        spellCheck={false}
        autoCorrect="off"
        autoCapitalize="off"
        onChange={(event) => setCustomTime(event.target.value)}
        // A time only counts once it's confirmed, so wandering off doesn't keep a laptop awake.
        onBlur={() => {
          if (document.hasFocus()) setCustomTime(null);
        }}
        onKeyDown={(event) => {
          if (act(event, "sheet.submit", () => keepAwakeForTyped(customTime))) return;
          act(event, "sheet.cancel", () => setCustomTime(null));
        }}
      />
    );
  }
  return (
    <Picker
      label={label}
      className="picker"
      value={null}
      disabled={!online}
      open={picking}
      onOpenChange={setPicking}
      onChange={chooseTime}
      trigger={
        <>
          {awakeTill === null ? "Off" : `${timeLeft(awakeTill - now)} left`}
          <Icon name="fold" />
        </>
      }
    >
      {KEEP_AWAKE_SUGGESTIONS.map((suggestion) => (
        <Choice key={suggestion} value={suggestion} label={`For ${suggestion}`} />
      ))}
      <Choice value={CUSTOM} label="For a custom time..." />
      {awakeTill !== null && <Choice value={STOP} label="Stop" />}
    </Picker>
  );
}
