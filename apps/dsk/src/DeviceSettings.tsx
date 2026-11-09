import { useEffect, useState } from "react";
import {
  type Command,
  type Device,
  errorText,
  type Provider,
  type ProviderCandidate,
  type ProviderModel,
  request,
} from "./engine";
import { act } from "./keybinds";
import { inPane } from "./keys";
import { defaultEffort, isOnline, modelLabel } from "./model";
import { hint, type Hint, useHints } from "./shortcuts";
import { Choice, Picker } from "./ui/Picker";
import { SettingsLayout } from "./ui/SettingsPage";
import { Keys } from "./ui/StatusIcon";

const SECTIONS = [{ id: "providers", label: "Providers" }];

/** A candidate, or a provider this daemon no longer knows how to run. */
function rowsOf(candidates: ProviderCandidate[], providers: Provider[]): ProviderCandidate[] {
  const gone = providers
    .filter((provider) => !candidates.some((c) => c.agent === provider.agent))
    .map((provider) => ({
      agent: provider.agent,
      name: provider.name,
      missing: "This device no longer has this agent",
    }));
  return [...candidates, ...gone];
}

function describe(provider: Provider): string {
  const models = provider.models.length === 1 ? "1 model" : `${provider.models.length} models`;
  return provider.version ? `${provider.version}, ${models}` : models;
}

const defaultModelOf = (provider: Provider): ProviderModel | undefined =>
  provider.models.find((m) => m.id === provider.default_model) ?? provider.models[0];

/** The settings of one device. They live on its daemon, so every client sees the same ones. */
export function DeviceSettings({
  device,
  onClose,
}: {
  device: Device;
  onClose: () => void;
}) {
  const machineId = device.state?.machine.id;
  const online = isOnline(device);
  const providers = device.state?.providers ?? [];
  const [candidates, setCandidates] = useState<ProviderCandidate[] | null>(null);
  const [busy, setBusy] = useState<Record<string, string>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [checkError, setCheckError] = useState<string | null>(null);
  const [cursor, setCursor] = useState(0);
  const [picking, setPicking] = useState<"model" | "effort" | null>(null);

  const rows = candidates ? rowsOf(candidates, providers) : [];
  const selected = Math.max(Math.min(cursor, rows.length - 1), 0);

  // Checking starts nothing. An agent only starts once it's added.
  useEffect(() => {
    if (!machineId || !online) return;
    let active = true;
    request(machineId, { command: "detect_providers" })
      .then((output) => {
        if (active && output.output === "provider_candidates") setCandidates(output.candidates);
      })
      .catch((reason) => {
        if (active) setCheckError(errorText(reason));
      });
    return () => {
      active = false;
    };
  }, [machineId, online]);

  const run = async (agent: string, doing: string, command: Command) => {
    if (!machineId || busy[agent]) return;
    setBusy((all) => ({ ...all, [agent]: doing }));
    setErrors(({ [agent]: _cleared, ...rest }) => rest);
    try {
      await request(machineId, command);
    } catch (reason) {
      setErrors((all) => ({ ...all, [agent]: errorText(reason) }));
    } finally {
      setBusy(({ [agent]: _done, ...rest }) => rest);
    }
  };

  const add = (agent: string, doing: string) =>
    run(agent, doing, { command: "add_provider", agent });
  const remove = (agent: string) =>
    run(agent, "Removing", { command: "remove_provider", agent });

  /** A new model may not have the current effort, so it gets one it does have. */
  const setDefaultModel = (provider: Provider, id: string) => {
    const model = provider.models.find((m) => m.id === id);
    const effort = model ? defaultEffort(provider, model) : undefined;
    void run(provider.agent, "Saving", {
      command: "set_provider_defaults",
      agent: provider.agent,
      model: id,
      effort: effort?.id ?? null,
    });
  };

  const setDefaultEffort = (provider: Provider, effort: string) => {
    void run(provider.agent, "Saving", {
      command: "set_provider_defaults",
      agent: provider.agent,
      model: defaultModelOf(provider)?.id ?? null,
      effort,
    });
  };

  const selectedRow = rows[selected];
  const selectedProvider = selectedRow && providers.find((p) => p.agent === selectedRow.agent);
  const rowHints: Hint[] = [];
  if (selectedProvider) {
    rowHints.push(hint("Refresh", "device.confirm"), hint("Remove", "device.remove"));
    if (selectedProvider.models.length > 0) rowHints.push(hint("Model", "device.pick_model"));
    if (defaultModelOf(selectedProvider)?.efforts.length) {
      rowHints.push(hint("Effort", "device.pick_effort"));
    }
  } else if (selectedRow && !selectedRow.missing) {
    rowHints.push(hint("Add", "device.confirm"));
  }
  useHints("pane", [
    ...rowHints,
    ...(rows.length > 0
      ? [
          hint("Move", "device.up", "device.down"),
          hint("First, last", "device.first", "device.last"),
          hint("Center", "device.center"),
        ]
      : []),
    hint("Close", "device.back"),
  ]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!inPane(event)) return;
      const moveBy = (step: number) => () =>
        setCursor(Math.max(Math.min(selected + step, rows.length - 1), 0));
      if (act(event, "device.up", moveBy(-1)) || act(event, "device.down", moveBy(1))) return;
      if (act(event, "device.first", () => setCursor(0))) return;
      if (act(event, "device.last", () => setCursor(rows.length - 1))) return;
      const center = () =>
        document.querySelector(".settings-provider.current")?.scrollIntoView({ block: "center" });
      if (act(event, "device.center", center)) return;
      const row = rows[selected];
      if (!row) return;
      const provider = providers.find((p) => p.agent === row.agent);
      const confirm = () => {
        if (provider || !row.missing) void add(row.agent, provider ? "Refreshing" : "Adding");
      };
      if (act(event, "device.confirm", confirm)) return;
      if (provider && act(event, "device.remove", () => void remove(row.agent))) return;
      if (provider?.models.length && act(event, "device.pick_model", () => setPicking("model"))) {
        return;
      }
      if (provider && defaultModelOf(provider)?.efforts.length) {
        act(event, "device.pick_effort", () => setPicking("effort"));
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  return (
    <SettingsLayout
      heading="Device settings"
      detail={device.state?.machine.name ?? device.url}
      sections={SECTIONS}
      section="providers"
      closeKeys="device.back"
      onClose={onClose}
    >
      <div className="pane settings">
        <div className="settings-in">
          {!online && <p className="empty">This device is offline</p>}
          {online && !candidates && !checkError && <p className="empty">Checking this device</p>}
          {checkError && <p className="error">{checkError}</p>}
          {rows.map((row, index) => {
            const provider = providers.find((p) => p.agent === row.agent);
            const current = index === selected;
            const doing = busy[row.agent];
            const canAdd = !provider && !row.missing;
            const model = provider && defaultModelOf(provider);
            const effort = provider && model && defaultEffort(provider, model);
            return (
              <div
                key={row.agent}
                className={`settings-provider ${current ? "current" : ""}`}
                onMouseEnter={() => setCursor(index)}
              >
                <div className="settings-row">
                  <span className="name">{row.name}</span>
                  <span className="meta">
                    {provider ? describe(provider) : (row.missing ?? "Available")}
                  </span>
                  {doing && <span className="meta">{doing}</span>}
                  {provider && (
                    <>
                      <button
                        className="button"
                        disabled={doing !== undefined}
                        onClick={() => void add(row.agent, "Refreshing")}
                      >
                        Refresh
                        {current && <Keys action="device.confirm" />}
                      </button>
                      <button
                        className="button"
                        disabled={doing !== undefined}
                        onClick={() => void remove(row.agent)}
                      >
                        Remove
                        {current && <Keys action="device.remove" />}
                      </button>
                    </>
                  )}
                  {canAdd && (
                    <button
                      className="button primary"
                      disabled={doing !== undefined}
                      onClick={() => void add(row.agent, "Adding")}
                    >
                      Add
                      {current && <Keys action="device.confirm" />}
                    </button>
                  )}
                </div>
                {provider && model && (
                  <div className="settings-defaults">
                    <div className="settings-default">
                      Default model
                      <Picker
                        label={`Default model for ${provider.name}`}
                        className="picker"
                        value={model.id}
                        open={current && picking === "model"}
                        onOpenChange={(open) => {
                          setCursor(index);
                          setPicking(open ? "model" : null);
                        }}
                        onChange={(id) => setDefaultModel(provider, id)}
                        finalFocus={false}
                        trigger={
                          <>
                            {modelLabel(provider, model)}
                            {current && <Keys action="device.pick_model" />}
                          </>
                        }
                      >
                        {provider.models.map((m) => (
                          <Choice key={m.id} value={m.id} label={modelLabel(provider, m)} />
                        ))}
                      </Picker>
                    </div>
                    {effort && (
                      <div className="settings-default">
                        Default effort
                        <Picker
                          label={`Default effort for ${provider.name}`}
                          className="picker"
                          value={effort.id}
                          open={current && picking === "effort"}
                          onOpenChange={(open) => {
                            setCursor(index);
                            setPicking(open ? "effort" : null);
                          }}
                          onChange={(id) => setDefaultEffort(provider, id)}
                          finalFocus={false}
                          trigger={
                            <>
                              {effort.name}
                              {current && <Keys action="device.pick_effort" />}
                            </>
                          }
                        >
                          {model.efforts.map((e) => (
                            <Choice key={e.id} value={e.id} label={e.name} />
                          ))}
                        </Picker>
                      </div>
                    )}
                  </div>
                )}
                {errors[row.agent] && <p className="error settings-error">{errors[row.agent]}</p>}
              </div>
            );
          })}
        </div>
      </div>
    </SettingsLayout>
  );
}
