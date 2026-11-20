import { useEffect, useState } from "react";
import {
  clientKey,
  type Command,
  type Device,
  errorText,
  type PairedClient,
  type Provider,
  type ProviderCandidate,
  type ProviderModel,
  request,
} from "./engine";
import { act } from "./keybinds";
import { McpSettings } from "./McpServers";
import {
  awakeUntil,
  defaultEffort,
  isOnline,
  KEEP_AWAKE_SUGGESTIONS,
  keepsAwake,
  modelLabel,
  parseDuration,
  timeLeft,
  withKeepAwakeToggled,
} from "./model";
import { isLoopback } from "./pathInput";
import { DEVICE_SECTIONS, sectionOf } from "./settingsNav";
import { Icon } from "./ui/Icon";
import { MenuButton } from "./ui/Menu";
import { Choice, Picker } from "./ui/Picker";
import {
  SettingRow,
  SettingsGroupBox,
  SettingsLayout,
  type SettingsNav,
  SettingsPage,
  Switch,
} from "./ui/SettingsPage";
import { useNow } from "./ui/useNow";

/** A candidate, or a provider this daemon no longer knows how to run. */
function rowsOf(candidates: ProviderCandidate[], providers: Provider[]): ProviderCandidate[] {
  const gone = providers
    .filter((provider) => !candidates.some((c) => c.agent === provider.agent))
    .map((provider) => ({
      agent: provider.agent,
      name: provider.name,
      missing: "No longer on this device",
    }));
  return [...candidates, ...gone];
}

function describe(provider: Provider): string {
  const models = provider.models.length === 1 ? "1 model" : `${provider.models.length} models`;
  return provider.version ? `${provider.version}, ${models}` : models;
}

const defaultModelOf = (provider: Provider): ProviderModel | undefined =>
  provider.models.find((m) => m.id === provider.default_model) ?? provider.models[0];

/** The settings of one device. They live on its daemon, so every client sees the same ones.
 * Its sections are in the same list as the app's. */
export function DeviceSettings({
  device,
  machineId,
  section: shown,
  nav,
  onPair,
  onForget,
}: {
  device: Device;
  machineId: string;
  section: string | undefined;
  nav: SettingsNav;
  /** Shows a code that lets another machine in. */
  onPair: () => void;
  /** Stops this app reaching the device. */
  onForget: () => void;
}) {
  const section = sectionOf(DEVICE_SECTIONS, shown).id;
  return (
    <SettingsLayout nav={nav} current={{ kind: "device_settings", machineId, section }}>
      {section === "general" && <General device={device} />}
      {section === "providers" && <Providers device={device} />}
      {section === "mcp" && <McpSettings device={device} />}
      {section === "pairing" && <Pairing device={device} onPair={onPair} onForget={onForget} />}
    </SettingsLayout>
  );
}

const CUSTOM = "custom";
const STOP = "stop";

/** The device's own name and power settings, each changed right in its row. */
function General({ device }: { device: Device }) {
  const state = device.state;
  const online = isOnline(device);
  const [error, setError] = useState<string | null>(null);
  // What's typed in the name's box, once something is. Until then it shows the name.
  const [name, setName] = useState<string | null>(null);
  // Typing a time of one's own, rather than choosing one of the suggestions.
  const [customTime, setCustomTime] = useState<string | null>(null);
  const [picking, setPicking] = useState(false);
  const now = useNow(state?.awake_until_ms !== undefined);
  const awakeTill = awakeUntil(state, now);

  if (!state) {
    return <SettingsPage>{!online && <p className="settings-note">This device is offline</p>}</SettingsPage>;
  }
  const deviceId = state.machine.id;

  const saveName = () => {
    const typed = name;
    setName(null);
    // Leaving the hostname it shows as it is isn't choosing it, so it can go on following it.
    if (typed === null || typed.trim() === state.machine.name || !online) return;
    setError(null);
    request(deviceId, { command: "rename_machine", name: typed }).catch((reason) =>
      setError(errorText(reason)),
    );
  };

  const toggleKeepAwake = () => {
    if (!online) return;
    setError(null);
    request(deviceId, {
      command: "set_device_settings",
      settings: withKeepAwakeToggled(state),
    }).catch((reason) => setError(errorText(reason)));
  };

  const keepAwakeFor = (seconds: number | null) => {
    if (!online) return;
    setError(null);
    request(deviceId, { command: "keep_awake_for", seconds }).catch((reason) =>
      setError(errorText(reason)),
    );
  };

  const keepAwakeForTyped = (typed: string) => {
    const seconds = parseDuration(typed);
    if (seconds === null) {
      setError("Type a time such as 2h, 90m or 1h30m, up to 7 days");
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

  return (
    <SettingsPage error={error}>
      {!online && <p className="settings-note">This device is offline</p>}
      <SettingsGroupBox title="Device">
        <SettingRow label="Name">
          <input
            className="settings-input"
            value={name ?? state.machine.name}
            disabled={!online}
            aria-label="Device name"
            placeholder="hostname"
            spellCheck={false}
            autoCorrect="off"
            autoCapitalize="off"
            onChange={(event) => setName(event.target.value)}
            // Switching to another app blurs it too, and that isn't being done with it.
            onBlur={() => {
              if (document.hasFocus()) saveName();
            }}
            onKeyDown={(event) => {
              // Done typing hands the keyboard to the row, so the row keys carry on from here.
              // Leaving the box is what saves.
              const box = event.currentTarget;
              const toRow = () => box.closest<HTMLElement>("[data-setting]")?.focus();
              if (act(event, "sheet.submit", toRow)) return;
              act(event, "sheet.cancel", () => {
                setName(null);
                // Reverted first, so the blur that follows has nothing to save.
                requestAnimationFrame(toRow);
              });
            }}
          />
        </SettingRow>
        <SettingRow
          label="Keep awake while agents work"
          // Left turns it off and right on, the way its switch slides.
          onStep={(direction) => {
            if ((direction > 0) !== keepsAwake(state)) toggleKeepAwake();
          }}
        >
          <Switch
            label="Keep awake while agents work"
            checked={keepsAwake(state)}
            disabled={!online}
            onChange={toggleKeepAwake}
          />
        </SettingRow>
        <SettingRow label="Keep awake for a while">
          {customTime !== null ? (
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
              // A time only counts once it's confirmed, so wandering off doesn't keep a laptop
              // awake.
              onBlur={() => {
                if (document.hasFocus()) setCustomTime(null);
              }}
              onKeyDown={(event) => {
                if (act(event, "sheet.submit", () => keepAwakeForTyped(customTime))) return;
                act(event, "sheet.cancel", () => setCustomTime(null));
              }}
            />
          ) : (
            <Picker
              label="Keep awake for a while"
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
          )}
        </SettingRow>
      </SettingsGroupBox>
    </SettingsPage>
  );
}

/** The agents this device can run, the added ones first. */
function Providers({ device }: { device: Device }) {
  const machineId = device.state?.machine.id;
  const online = isOnline(device);
  const providers = device.state?.providers ?? [];
  const [candidates, setCandidates] = useState<ProviderCandidate[] | null>(null);
  const [busy, setBusy] = useState<Record<string, string>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [checkError, setCheckError] = useState<string | null>(null);

  const all = candidates ? rowsOf(candidates, providers) : [];
  const isAdded = (row: ProviderCandidate) => providers.some((p) => p.agent === row.agent);
  const added = all.filter(isAdded);
  const available = all.filter((row) => !isAdded(row));

  // Checking starts nothing. An agent only starts once it's added.
  useEffect(() => {
    if (!machineId || !online) return;
    let shown = true;
    request(machineId, { command: "detect_providers" })
      .then((output) => {
        if (shown && output.output === "provider_candidates") setCandidates(output.candidates);
      })
      .catch((reason) => {
        if (shown) setCheckError(errorText(reason));
      });
    return () => {
      shown = false;
    };
  }, [machineId, online]);

  const run = async (agent: string, label: string, command: Command) => {
    if (!machineId || busy[agent]) return;
    setBusy((current) => ({ ...current, [agent]: label }));
    setErrors(({ [agent]: _cleared, ...rest }) => rest);
    try {
      await request(machineId, command);
    } catch (reason) {
      setErrors((current) => ({ ...current, [agent]: errorText(reason) }));
    } finally {
      setBusy(({ [agent]: _done, ...rest }) => rest);
    }
  };

  const add = (target: ProviderCandidate) => {
    const has = isAdded(target);
    if (has || !target.missing) {
      void run(target.agent, has ? "Refreshing" : "Adding", { command: "add_provider", agent: target.agent });
    }
  };
  const remove = (target: ProviderCandidate) => {
    if (isAdded(target)) {
      void run(target.agent, "Removing", { command: "remove_provider", agent: target.agent });
    }
  };

  /** A new model may not have the current effort, so it gets one it does have. */
  const setDefaultModel = (target: Provider, id: string) => {
    const chosen = target.models.find((m) => m.id === id);
    const fitting = chosen ? defaultEffort(target, chosen) : undefined;
    void run(target.agent, "Saving", {
      command: "set_provider_defaults",
      agent: target.agent,
      model: id,
      effort: fitting?.id ?? null,
    });
  };

  const setDefaultEffort = (target: Provider, id: string) => {
    void run(target.agent, "Saving", {
      command: "set_provider_defaults",
      agent: target.agent,
      model: defaultModelOf(target)?.id ?? null,
      effort: id,
    });
  };

  const addedRow = (target: ProviderCandidate) => {
    const own = providers.find((p) => p.agent === target.agent);
    if (!own) return null;
    const ownModel = defaultModelOf(own);
    const ownEffort = ownModel ? defaultEffort(own, ownModel) : undefined;
    const doing = busy[target.agent];
    const usable = online && doing === undefined;
    return (
      <SettingRow
        key={target.agent}
        label={target.name}
        // Left and right go through the default model, as they do through any dropdown's choices.
        onStep={(direction) => {
          const at = own.models.findIndex((m) => m.id === ownModel?.id);
          const next = own.models[at + direction];
          if (usable && next) setDefaultModel(own, next.id);
        }}
        meta={<span className="meta">{doing ?? describe(own)}</span>}
        error={errors[target.agent]}
        quiet={
          <MenuButton
            label={`Actions for ${own.name}`}
            className="icon-button"
            disabled={!usable}
            groups={[
              [
                { id: "refresh", label: "Refresh", run: () => add(target) },
                { id: "remove", label: "Remove", run: () => remove(target) },
              ],
            ]}
          >
            <Icon name="more" />
          </MenuButton>
        }
      >
        {ownModel && (
          <ProviderPicker
            label={`Default model for ${own.name}`}
            value={ownModel.id}
            disabled={!usable}
            trigger={modelLabel(own, ownModel)}
            choices={own.models.map((m) => ({ value: m.id, label: modelLabel(own, m) }))}
            onChange={(id) => setDefaultModel(own, id)}
          />
        )}
        {ownModel && ownEffort && (
          <ProviderPicker
            label={`Default effort for ${own.name}`}
            value={ownEffort.id}
            disabled={!usable}
            trigger={ownEffort.name}
            choices={ownModel.efforts.map((e) => ({ value: e.id, label: e.name }))}
            onChange={(id) => setDefaultEffort(own, id)}
          />
        )}
      </SettingRow>
    );
  };

  const availableRow = (target: ProviderCandidate) => {
    const doing = busy[target.agent];
    return (
      <SettingRow
        key={target.agent}
        label={target.name}
        meta={doing || target.missing ? <span className="meta">{doing ?? target.missing}</span> : undefined}
        error={errors[target.agent]}
      >
        {!target.missing && (
          <button
            className="button small"
            disabled={!online || doing !== undefined}
            onClick={() => add(target)}
          >
            Add
          </button>
        )}
      </SettingRow>
    );
  };

  return (
    <SettingsPage error={checkError}>
      {!online && <p className="settings-note">This device is offline</p>}
      {online && !candidates && !checkError && <p className="settings-note">Checking this device</p>}
      {added.length > 0 && <SettingsGroupBox title="Added">{added.map(addedRow)}</SettingsGroupBox>}
      {available.length > 0 && (
        <SettingsGroupBox title="Available">{available.map(availableRow)}</SettingsGroupBox>
      )}
      {candidates && all.length === 0 && <p className="settings-note">No agents found on this device</p>}
    </SettingsPage>
  );
}

/** A dropdown of a provider's defaults, which keeps whether it's open to itself. */
function ProviderPicker({
  label,
  value,
  disabled,
  trigger,
  choices,
  onChange,
}: {
  label: string;
  value: string;
  disabled: boolean;
  trigger: string;
  choices: { value: string; label: string }[];
  onChange: (value: string) => void;
}) {
  const [open, setOpen] = useState(false);
  return (
    <Picker
      label={label}
      className="picker"
      value={value}
      disabled={disabled}
      open={open}
      onOpenChange={setOpen}
      onChange={onChange}
      trigger={trigger}
    >
      {choices.map((choice) => (
        <Choice key={choice.value} value={choice.value} label={choice.label} />
      ))}
    </Picker>
  );
}

/** The machines that may connect to this device. Pairing starts only on the device itself, and
 * on another device this app's own row is how it stops reaching that device. */
function Pairing({
  device,
  onPair,
  onForget,
}: {
  device: Device;
  onPair: () => void;
  onForget: () => void;
}) {
  const state = device.state;
  const online = isOnline(device);
  const local = isLoopback(device.url);
  const clients: PairedClient[] = state?.paired_clients ?? [];
  const [error, setError] = useState<string | null>(null);
  const [ownKey, setOwnKey] = useState<string | null>(null);
  const isThisApp = (c: PairedClient) => c.key === ownKey;

  useEffect(() => {
    clientKey()
      .then(setOwnKey)
      .catch(() => setOwnKey(null));
  }, []);

  const remove = (target: PairedClient) => {
    if (!state || !online) return;
    if (isThisApp(target)) {
      onForget();
      return;
    }
    setError(null);
    request(state.machine.id, { command: "remove_paired_client", key: target.key }).catch(
      (reason) => setError(errorText(reason)),
    );
  };

  return (
    <SettingsPage error={error}>
      {!online && <p className="settings-note">This device is offline</p>}
      <SettingsGroupBox
        title="Machines that can connect"
        actions={
          local && (
            <button
              className={`button ${clients.length === 0 ? "primary" : ""}`}
              disabled={!online}
              onClick={onPair}
            >
              Pair
            </button>
          )
        }
      >
        {clients.length === 0 && <p className="settings-note">No machines paired</p>}
        {clients.map((c) => (
          <SettingRow
            key={c.key}
            label={isThisApp(c) ? `${c.name} (this app)` : c.name}
            meta={<span className="meta">Paired {new Date(c.paired_at_ms).toLocaleDateString()}</span>}
            quiet={
              <button
                className="button small"
                disabled={!online}
                title={
                  isThisApp(c)
                    ? "Stops this app reaching this device."
                    : "Disconnects it, and it can only come back by pairing again."
                }
                onClick={() => remove(c)}
              >
                Remove
              </button>
            }
          />
        ))}
      </SettingsGroupBox>
    </SettingsPage>
  );
}
