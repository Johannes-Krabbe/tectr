import { type ReactNode, type RefObject, useEffect, useRef, useState } from "react";
import { type Device, errorText, type PermissionMode } from "./engine";
import { act, type ActionId, textBoxKey, titled } from "./keybinds";
import { defaultEffort, isOnline, modelLabel, MODES } from "./model";
import { hint, type Hint, useHints } from "./shortcuts";
import { Choice, Picker } from "./ui/Picker";
import { Keys } from "./ui/StatusIcon";
import { useVim, VimModeLabel } from "./VimBox";

/** What's been chosen and typed for a new agent so far. Only what was changed is here: the
 * rest comes from the provider's defaults, and a choice that doesn't apply to the current
 * device or model is ignored. It belongs to this client and isn't saved anywhere, so
 * restarting clears it. */
export interface AgentDraft {
  text?: string;
  agent?: string;
  model?: string;
  effort?: string;
  mode?: PermissionMode;
}

/** Everything starting the agent needs, with the defaults filled in. */
export interface AgentChoice {
  agent: string;
  mode: PermissionMode;
  model: string | null;
  effort: string | null;
  text: string;
}

/** What a screen's own picker needs to open from its key and close back to the message box. */
export interface PickerControl {
  finalFocus: RefObject<HTMLTextAreaElement | null>;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/** A picker a screen adds above the message box, such as the new topic's workspace. */
export interface ExtraPicker {
  name: string;
  label: string;
  action: ActionId;
}

const AGENT_PICKERS: { name: string; label: string; action: ActionId }[] = [
  { name: "provider", label: "Provider", action: "new_topic.pick_provider" },
  { name: "model", label: "Model", action: "new_topic.pick_model" },
  { name: "effort", label: "Effort", action: "new_topic.pick_effort" },
  { name: "mode", label: "Permission mode", action: "new_topic.pick_mode" },
];

/** The first prompt for a new agent and what it runs on. The new topic screen and a new tab
 * share it, so starting an agent looks and works the same wherever it happens. */
export function AgentForm({
  device,
  draft,
  onDraft,
  ready = true,
  autoFocus = false,
  extraPickers = [],
  selects,
  onStart,
  onCancel,
  onSettings,
}: {
  device: Device | undefined;
  draft: AgentDraft;
  onDraft: (change: AgentDraft) => void;
  /** Whatever else the screen needs before it can start, such as a workspace. */
  ready?: boolean;
  autoFocus?: boolean;
  extraPickers?: ExtraPicker[];
  selects?: (control: (name: string) => PickerControl) => ReactNode;
  /** Starts the agent. A thrown error stays on the form, so it can be tried again. */
  onStart: (choice: AgentChoice) => Promise<void>;
  onCancel: () => void;
  onSettings: (machineId: string) => void;
}) {
  const text = draft.text ?? "";
  const [picking, setPicking] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const input = useRef<HTMLTextAreaElement>(null);
  const vim = useVim(input);

  const machineId = device?.state?.machine.id;
  const online = device !== undefined && isOnline(device);
  const providers = device?.state?.providers ?? [];
  const provider = providers.find((p) => p.agent === draft.agent) ?? providers[0];
  const model =
    provider?.models.find((m) => m.id === draft.model) ??
    provider?.models.find((m) => m.id === provider.default_model) ??
    provider?.models[0];
  const efforts = model?.efforts ?? [];
  const effort =
    efforts.find((e) => e.id === draft.effort) ??
    (provider && model ? defaultEffort(provider, model) : undefined);
  const modes = MODES.filter((option) => provider?.modes.includes(option.mode));
  const mode =
    modes.find((option) => option.mode === draft.mode) ??
    modes.find((option) => option.mode === "auto") ??
    modes[0];

  const canStart =
    ready && online && provider !== undefined && mode !== undefined && text.trim() !== "" && !busy;

  const control = (name: string): PickerControl => ({
    finalFocus: input,
    open: picking === name,
    onOpenChange: (open) => setPicking(open ? name : null),
  });

  const shown = new Set(extraPickers.map((p) => p.name));
  if (provider) shown.add("provider");
  if (model) shown.add("model");
  if (efforts.length > 0) shown.add("effort");
  if (mode) shown.add("mode");
  const pickers = [...extraPickers, ...AGENT_PICKERS];
  const pickerHints: Hint[] = pickers
    .filter((p) => shown.has(p.name))
    .map((p) => hint(p.label, p.action));
  useHints("pane", [...pickerHints, hint("Sidebar", "pane.back")]);
  useHints("typing", [
    ...(canStart ? [hint("Start", "new_topic.start")] : []),
    hint("New line", "new_topic.newline"),
    hint("Cancel", "new_topic.cancel"),
    ...pickerHints,
  ]);

  // Read through a ref, because the listener is added once.
  const keys = useRef(pickers);
  keys.current = pickers;
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (document.querySelector(".sheet")) return;
      keys.current.some(({ name, action }) =>
        act(event, action, () => setPicking((open) => (open === name ? null : name))),
      );
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const start = async () => {
    if (!canStart) return;
    setBusy(true);
    setError(null);
    try {
      await onStart({
        agent: provider.agent,
        mode: mode.mode,
        model: model?.id ?? null,
        effort: effort?.id ?? null,
        text: text.trim(),
      });
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      className="pane create"
      onSubmit={(event) => {
        event.preventDefault();
        void start();
      }}
    >
      <div className="create-in">
        {selects?.(control)}
        <div className="composer">
          <textarea
            ref={input}
            autoFocus={autoFocus}
            readOnly={busy}
            value={text}
            placeholder="What should the agent do?"
            aria-label="First prompt"
            autoCorrect="off"
            autoCapitalize="off"
            onChange={(event) => onDraft({ text: event.target.value })}
            {...vim.props}
            onKeyDown={(event) => {
              if (vim.onKeyDown(event)) return;
              textBoxKey(
                event,
                ["new_topic.start", () => void start()],
                "new_topic.newline",
                ["new_topic.cancel", onCancel],
              );
            }}
          />
          <div className="composer-pickers">
            {provider ? (
              <>
                <Picker
                  {...control("provider")}
                  label="Provider"
                  title={titled("Provider", "new_topic.pick_provider")}
                  className="picker"
                  value={provider.agent}
                  onChange={(agent) => onDraft({ agent })}
                  trigger={provider.name}
                >
                  {providers.map((p) => (
                    <Choice key={p.agent} value={p.agent} label={p.name} />
                  ))}
                </Picker>
                {model && (
                  <Picker
                    {...control("model")}
                    label="Model"
                    title={titled("Model", "new_topic.pick_model")}
                    className="picker"
                    value={model.id}
                    onChange={(model) => onDraft({ model })}
                    trigger={modelLabel(provider, model)}
                  >
                    {provider.models.map((m) => (
                      <Choice key={m.id} value={m.id} label={modelLabel(provider, m)} />
                    ))}
                  </Picker>
                )}
                {efforts.length > 0 && (
                  <Picker
                    {...control("effort")}
                    label="Effort"
                    title={titled("Effort", "new_topic.pick_effort")}
                    className="picker"
                    value={effort?.id ?? null}
                    onChange={(effort) => onDraft({ effort })}
                    trigger={`${effort?.name} effort`}
                  >
                    {efforts.map((e) => (
                      <Choice key={e.id} value={e.id} label={e.name} />
                    ))}
                  </Picker>
                )}
                {mode && (
                  <Picker
                    {...control("mode")}
                    label="Permission mode"
                    title={titled("Permission mode", "new_topic.pick_mode")}
                    className="picker"
                    value={mode.mode}
                    onChange={(value) => {
                      const chosen = modes.find((option) => option.mode === value);
                      if (chosen) onDraft({ mode: chosen.mode });
                    }}
                    trigger={mode.label}
                  >
                    {modes.map((option) => (
                      <Choice key={option.mode} value={option.mode} label={option.label} />
                    ))}
                  </Picker>
                )}
              </>
            ) : (
              <button
                type="button"
                className="picker"
                disabled={!online || machineId === undefined}
                onClick={() => machineId && onSettings(machineId)}
              >
                Add a provider
              </button>
            )}
            <VimModeLabel mode={vim.shown} />
          </div>
        </div>
        <button className="button primary create-start" type="submit" disabled={!canStart}>
          {busy ? "Starting" : "Start"}
          <Keys action="new_topic.start" />
        </button>
        {device?.state && !online && (
          <p className="error">{device.state.machine.name} is offline</p>
        )}
        {error && <p className="error">{error}</p>}
      </div>
    </form>
  );
}
