import { type ReactNode, useEffect, useRef, useState } from "react";
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
import { act, type ActionId } from "./keybinds";
import { inPane } from "./keys";
import { McpSettings } from "./McpServers";
import { defaultEffort, isOnline, keepsAwake, modelLabel, withKeepAwakeToggled } from "./model";
import { isLoopback } from "./pathInput";
import { DEVICE_SECTIONS, sectionOf } from "./settingsNav";
import { hint, type Hint, SETTINGS_MOVES, useHints } from "./shortcuts";
import { Choice, Picker } from "./ui/Picker";
import {
  GoInHint,
  listMoves,
  SettingsLayout,
  type SettingsNav,
  SettingsPage,
} from "./ui/SettingsPage";
import { Keys } from "./ui/StatusIcon";

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

/** Selected rows scroll into view as the cursor moves. */
export function useScrollToCurrent(selected: number, active: boolean) {
  useEffect(() => {
    if (active) document.querySelector(".keybind.current")?.scrollIntoView({ block: "nearest" });
  }, [selected, active]);
}

/** One row of a device settings list, which takes the cursor and the keyboard when the
 * pointer moves over it. */
export function Row({
  current,
  onPoint,
  children,
}: {
  current: boolean;
  onPoint: () => void;
  children: ReactNode;
}) {
  return (
    <div
      className={`keybind ${current ? "current" : ""}`}
      // Moving, rather than entering: a row scrolling under a pointer that isn't moving must
      // not take the keyboard or the selection.
      onMouseMove={(event) => {
        if (event.movementX !== 0 || event.movementY !== 0) onPoint();
      }}
    >
      {children}
    </div>
  );
}

// Focus stays off controls in a row, so Enter does what the settings say rather than pressing them.
export const keepFocus = (event: React.MouseEvent) => event.preventDefault();

/** The settings of one device. They live on its daemon, so every client sees the same ones.
 * Its sections are in the same list as the app's. */
export function DeviceSettings({
  device,
  machineId,
  section: shown,
  nav,
  renaming,
  onRename,
  onRenameEnd,
  onPair,
  onForget,
}: {
  device: Device;
  machineId: string;
  section: string | undefined;
  nav: SettingsNav;
  /** The name is being edited in place, which the palette can start from anywhere. */
  renaming: boolean;
  onRename: () => void;
  onRenameEnd: () => void;
  /** Shows a code that lets another machine in. */
  onPair: () => void;
  /** Stops this app reaching the device. */
  onForget: () => void;
}) {
  const section = sectionOf(DEVICE_SECTIONS, shown).id;
  // Whether the section has the keyboard, rather than the list of sections.
  const [inside, setInside] = useState(false);

  useEffect(() => {
    if (renaming) setInside(true);
  }, [renaming]);

  const props = {
    device,
    active: inside,
    onActivate: () => setInside(true),
    onLeave: () => {
      setInside(false);
      // Going back to the list of sections drops an unsaved name.
      if (renaming) onRenameEnd();
    },
  };

  return (
    <SettingsLayout
      nav={nav}
      current={{ kind: "device_settings", machineId, section }}
      inside={inside}
      onInside={(next) => (next ? setInside(true) : props.onLeave())}
    >
      {section === "general" && (
        <General {...props} renaming={renaming} onRename={onRename} onRenameEnd={onRenameEnd} />
      )}
      {section === "providers" && <Providers {...props} />}
      {section === "mcp" && <McpSettings {...props} />}
      {section === "pairing" && <Pairing {...props} onPair={onPair} onForget={onForget} />}
    </SettingsLayout>
  );
}

interface SectionProps {
  device: Device;
  /** This section has the keyboard, rather than the list of sections. */
  active: boolean;
  onActivate: () => void;
  onLeave: () => void;
}

interface GeneralRow {
  id: string;
  label: string;
  value: string;
  action: string;
  role?: "switch";
  run: () => void;
}

/** The device's own name and power settings. The name is edited right in its row. */
function General({
  device,
  active,
  onActivate,
  onLeave,
  renaming,
  onRename,
  onRenameEnd,
}: SectionProps & { renaming: boolean; onRename: () => void; onRenameEnd: () => void }) {
  const state = device.state;
  const online = isOnline(device);
  const [cursor, setCursor] = useState(0);
  const [error, setError] = useState<string | null>(null);
  // What's typed, once something is. Until then the box shows the name, so focusing selects it.
  const [draft, setDraft] = useState<string | null>(null);
  // Saving unmounts the input, whose blur would otherwise save a second time.
  const finished = useRef(false);
  const keepAwake = state ? keepsAwake(state) : true;
  const editing = renaming && online && state !== undefined;

  useEffect(() => {
    if (!renaming) return;
    finished.current = false;
    setDraft(null);
    setCursor(0);
  }, [renaming]);

  const endRename = (save: boolean) => {
    if (finished.current) return;
    finished.current = true;
    onRenameEnd();
    // Leaving the hostname it shows as it is isn't choosing it, so it can go on following it.
    if (!save || !state || draft === null || draft.trim() === state.machine.name) return;
    setError(null);
    request(state.machine.id, { command: "rename_machine", name: draft }).catch((reason) =>
      setError(errorText(reason)),
    );
  };

  const toggleKeepAwake = () => {
    if (!state || !online) return;
    setError(null);
    request(state.machine.id, {
      command: "set_device_settings",
      settings: withKeepAwakeToggled(state),
    }).catch((reason) => setError(errorText(reason)));
  };

  const rows: GeneralRow[] = state
    ? [
        {
          id: "name",
          label: "Name",
          value: state.machine.name,
          action: "Rename",
          run: onRename,
        },
        {
          id: "keep_awake",
          label: "Keep awake while agents work",
          value: keepAwake ? "On" : "Off",
          action: "Switch",
          role: "switch",
          run: toggleKeepAwake,
        },
      ]
    : [];
  const selected = Math.max(Math.min(cursor, rows.length - 1), 0);
  const row: GeneralRow | undefined = rows[selected];

  useScrollToCurrent(selected, active);
  useHints(
    "pane",
    editing
      ? [hint("Save", "sheet.submit"), hint("Cancel", "sheet.cancel")]
      : active
        ? [...(row && online ? [hint(row.action, "settings.edit")] : []), ...SETTINGS_MOVES]
        : null,
  );

  useEffect(() => {
    if (!active || editing) return;
    const onKey = (event: KeyboardEvent) => {
      if (!inPane(event)) return;
      const commands: [ActionId, () => void][] = listMoves(selected, rows.length, setCursor, onLeave);
      if (online) {
        if (row) commands.push(["settings.edit", row.run]);
        // The palette shows these keys for this device, so they work wherever the row is.
        commands.push(["device.rename", onRename], ["device.toggle_keep_awake", toggleKeepAwake]);
      }
      commands.some(([id, run]) => act(event, id, run));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const footer = editing ? (
    <>
      <span className="settings-actions-what" />
      <button className="button primary" onMouseDown={keepFocus} onClick={() => endRename(true)}>
        Save
        <Keys action="sheet.submit" />
      </button>
      <button className="button" onMouseDown={keepFocus} onClick={() => endRename(false)}>
        Cancel
        <Keys action="sheet.cancel" />
      </button>
    </>
  ) : active && row ? (
      <>
        <span className="settings-actions-what" />
        <button className="button" disabled={!online} onClick={row.run}>
          {row.action}
          <Keys action="settings.edit" />
        </button>
      </>
    ) : (
      <GoInHint />
    );

  return (
    <SettingsPage error={error} footer={footer}>
      {!online && <p className="settings-note">This device is offline</p>}
      <section className="settings-group">
        <h3 className="settings-group-title">Device</h3>
        <div className="keybinds">
          {rows.map((r, index) => {
            const current = active && index === selected;
            return (
              <Row
                key={r.id}
                current={current}
                onPoint={() => {
                  setCursor(index);
                  onActivate();
                }}
              >
                <span className="name">{r.label}</span>
                {editing && r.id === "name" ? (
                  <input
                    className="settings-input"
                    autoFocus
                    value={draft ?? state?.machine.name ?? ""}
                    aria-label="Device name"
                    placeholder="hostname"
                    spellCheck={false}
                    autoCorrect="off"
                    autoCapitalize="off"
                    onFocus={(event) => event.target.select()}
                    onChange={(event) => setDraft(event.target.value)}
                    // Switching to another app blurs it too, and that isn't being done with it.
                    onBlur={() => {
                      if (document.hasFocus()) endRename(true);
                    }}
                    onKeyDown={(event) => {
                      if (act(event, "sheet.submit", () => endRename(true))) return;
                      if (act(event, "sheet.cancel", () => endRename(false))) return;
                    }}
                  />
                ) : (
                <button
                  className="picker"
                  tabIndex={-1}
                  disabled={!online}
                  role={r.role}
                  aria-checked={r.role ? r.value === "On" : undefined}
                  aria-label={r.label}
                  onMouseDown={keepFocus}
                  onClick={r.run}
                >
                  {r.value}
                  {current && online && <Keys action="settings.edit" />}
                </button>
                )}
              </Row>
            );
          })}
        </div>
      </section>
    </SettingsPage>
  );
}

/** The agents this device can run, the added ones first. */
function Providers({ device, active, onActivate, onLeave }: SectionProps) {
  const machineId = device.state?.machine.id;
  const online = isOnline(device);
  const providers = device.state?.providers ?? [];
  const [candidates, setCandidates] = useState<ProviderCandidate[] | null>(null);
  const [busy, setBusy] = useState<Record<string, string>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [checkError, setCheckError] = useState<string | null>(null);
  const [cursor, setCursor] = useState(0);
  const [picking, setPicking] = useState<"model" | "effort" | null>(null);

  const all = candidates ? rowsOf(candidates, providers) : [];
  const isAdded = (row: ProviderCandidate) => providers.some((p) => p.agent === row.agent);
  const added = all.filter(isAdded);
  const available = all.filter((row) => !isAdded(row));
  // The cursor moves through both groups as one list.
  const rows = [...added, ...available];
  const selected = Math.max(Math.min(cursor, rows.length - 1), 0);
  const row: ProviderCandidate | undefined = rows[selected];
  const provider = row && providers.find((p) => p.agent === row.agent);
  const model = provider && defaultModelOf(provider);
  const doing = row && busy[row.agent];

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

  const canPickModel = (provider?.models.length ?? 0) > 0;
  const canPickEffort = (model?.efforts.length ?? 0) > 0;
  const confirmLabel = provider ? "Refresh" : row && !row.missing ? "Add" : null;

  const rowHints: Hint[] = [];
  if (row && online) {
    if (confirmLabel) rowHints.push(hint(confirmLabel, "device.confirm"));
    if (provider) rowHints.push(hint("Remove", "device.remove"));
    if (canPickModel) rowHints.push(hint("Model", "device.pick_model"));
    if (canPickEffort) rowHints.push(hint("Effort", "device.pick_effort"));
  }
  useHints("pane", active ? [...rowHints, ...SETTINGS_MOVES] : null);
  useScrollToCurrent(selected, active);

  useEffect(() => {
    if (!active || picking) return;
    const onKey = (event: KeyboardEvent) => {
      if (!inPane(event)) return;
      const commands: [ActionId, () => void][] = listMoves(selected, rows.length, setCursor, onLeave);
      if (row && online) {
        commands.push(["device.confirm", () => add(row)], ["device.remove", () => remove(row)]);
        if (canPickModel) commands.push(["device.pick_model", () => setPicking("model")]);
        if (canPickEffort) commands.push(["device.pick_effort", () => setPicking("effort")]);
      }
      commands.some(([id, fn]) => act(event, id, fn));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const footer =
    active && row ? (
      <>
        {errors[row.agent] ? (
          <span className="settings-actions-what error">{errors[row.agent]}</span>
        ) : (
          <span className="settings-actions-what">
            {doing ?? (provider ? `${row.name}, ${describe(provider)}` : (row.missing ?? row.name))}
          </span>
        )}
        {confirmLabel && (
          <button
            className={`button ${provider ? "" : "primary"}`}
            disabled={!online || doing !== undefined}
            onClick={() => add(row)}
          >
            {confirmLabel}
            <Keys action="device.confirm" />
          </button>
        )}
        {provider && (
          <button className="button" disabled={!online || doing !== undefined} onClick={() => remove(row)}>
            Remove
            <Keys action="device.remove" />
          </button>
        )}
      </>
    ) : (
      <GoInHint />
    );

  const renderRow = (target: ProviderCandidate) => {
    const index = rows.indexOf(target);
    const current = active && index === selected;
    const own = providers.find((p) => p.agent === target.agent);
    const ownModel = own && defaultModelOf(own);
    const ownEffort = own && ownModel ? defaultEffort(own, ownModel) : undefined;
    const label = busy[target.agent];
    const point = () => {
      setCursor(index);
      onActivate();
    };
    const onOpenChange = (which: "model" | "effort") => (open: boolean) => {
      point();
      setPicking(open ? which : null);
    };
    return (
      <Row key={target.agent} current={current} onPoint={point}>
        <span className="name">{target.name}</span>
        {label ? (
          <span className="meta">{label}</span>
        ) : errors[target.agent] ? (
          <span className="meta bad">Failed</span>
        ) : (
          <span className="meta">{own ? describe(own) : target.missing}</span>
        )}
        {own && ownModel && (
          <Picker
            label={`Default model for ${own.name}`}
            className="picker"
            value={ownModel.id}
            open={current && picking === "model"}
            onOpenChange={onOpenChange("model")}
            onChange={(id) => setDefaultModel(own, id)}
            finalFocus={false}
            trigger={
              <>
                {modelLabel(own, ownModel)}
                {current && <Keys action="device.pick_model" />}
              </>
            }
          >
            {own.models.map((m) => (
              <Choice key={m.id} value={m.id} label={modelLabel(own, m)} />
            ))}
          </Picker>
        )}
        {own && ownModel && ownEffort && (
          <Picker
            label={`Default effort for ${own.name}`}
            className="picker"
            value={ownEffort.id}
            open={current && picking === "effort"}
            onOpenChange={onOpenChange("effort")}
            onChange={(id) => setDefaultEffort(own, id)}
            finalFocus={false}
            trigger={
              <>
                {ownEffort.name}
                {current && <Keys action="device.pick_effort" />}
              </>
            }
          >
            {ownModel.efforts.map((e) => (
              <Choice key={e.id} value={e.id} label={e.name} />
            ))}
          </Picker>
        )}
      </Row>
    );
  };

  return (
    <SettingsPage error={checkError} footer={footer}>
      {!online && <p className="settings-note">This device is offline</p>}
      {online && !candidates && !checkError && <p className="settings-note">Checking this device</p>}
      {added.length > 0 && (
        <section className="settings-group">
          <h3 className="settings-group-title">Added</h3>
          <div className="keybinds">{added.map(renderRow)}</div>
        </section>
      )}
      {available.length > 0 && (
        <section className="settings-group">
          <h3 className="settings-group-title">Available</h3>
          <div className="keybinds">{available.map(renderRow)}</div>
        </section>
      )}
      {candidates && rows.length === 0 && <p className="settings-note">No agents found on this device</p>}
    </SettingsPage>
  );
}

/** The machines that may connect to this device. Pairing starts only on the device itself, and
 * on another device this app's own row is how it stops reaching that device. */
function Pairing({
  device,
  active,
  onActivate,
  onLeave,
  onPair,
  onForget,
}: SectionProps & { onPair: () => void; onForget: () => void }) {
  const state = device.state;
  const online = isOnline(device);
  const local = isLoopback(device.url);
  const canPair = online && local;
  const clients: PairedClient[] = state?.paired_clients ?? [];
  const [cursor, setCursor] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [ownKey, setOwnKey] = useState<string | null>(null);
  const selected = Math.max(Math.min(cursor, clients.length - 1), 0);
  const client: PairedClient | undefined = clients[selected];
  const isThisApp = (c: PairedClient) => c.key === ownKey;

  useEffect(() => {
    clientKey()
      .then(setOwnKey)
      .catch(() => setOwnKey(null));
  }, []);

  const remove = (target: PairedClient | undefined) => {
    if (!target || !state || !online) return;
    if (isThisApp(target)) {
      onForget();
      return;
    }
    setError(null);
    request(state.machine.id, { command: "remove_paired_client", key: target.key }).catch(
      (reason) => setError(errorText(reason)),
    );
  };

  useScrollToCurrent(selected, active);
  useHints(
    "pane",
    active
      ? [
          ...(canPair ? [hint("Pair", "pairing.add")] : []),
          ...(client && online ? [hint("Remove", "pairing.remove")] : []),
          ...SETTINGS_MOVES,
        ]
      : null,
  );

  useEffect(() => {
    if (!active) return;
    const onKey = (event: KeyboardEvent) => {
      if (!inPane(event)) return;
      const commands: [ActionId, () => void][] = listMoves(selected, clients.length, setCursor, onLeave);
      if (canPair) commands.push(["pairing.add", onPair]);
      if (online) commands.push(["pairing.remove", () => remove(client)]);
      if (!local) commands.push(["pairing.forget", onForget]);
      commands.some(([id, run]) => act(event, id, run));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const about = !client
    ? undefined
    : isThisApp(client)
      ? "Stops this app reaching this device."
      : "Disconnects it, and it can only come back by pairing again.";
  const footer =
    active && client ? (
      <>
        <span className="settings-actions-what">{about}</span>
        <button className="button" disabled={!online} onClick={() => remove(client)}>
          Remove
          <Keys action="pairing.remove" />
        </button>
      </>
    ) : (
      <GoInHint />
    );

  return (
    <SettingsPage error={error} footer={footer}>
      {!online && <p className="settings-note">This device is offline</p>}
      <section className="settings-group">
        <div className="settings-group-head">
          <h3 className="settings-group-title">Machines that can connect</h3>
          {local && (
            <button
              className={`button ${clients.length === 0 ? "primary" : ""}`}
              disabled={!canPair}
              onMouseDown={keepFocus}
              onClick={onPair}
            >
              Pair
              {active && <Keys action="pairing.add" />}
            </button>
          )}
        </div>
        {clients.length === 0 && <p className="settings-note">No machines paired</p>}
        <div className="keybinds">
          {clients.map((c, index) => (
            <Row
              key={c.key}
              current={active && index === selected}
              onPoint={() => {
                setCursor(index);
                onActivate();
              }}
            >
              <span className="name">{isThisApp(c) ? `${c.name} (this app)` : c.name}</span>
              <span className="meta">
                Paired {new Date(c.paired_at_ms).toLocaleDateString()}
              </span>
            </Row>
          ))}
        </div>
      </section>
    </SettingsPage>
  );
}
