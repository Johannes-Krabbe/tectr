import { type RefObject, useState } from "react";
import {
  type AgentOption,
  type Command,
  errorText,
  type PermissionMode,
  type Provider,
  request,
  type Session,
} from "./engine";
import { titled } from "./keybinds";
import {
  changedOption,
  effortForModel,
  modelLabel,
  MODES,
  settingsLabel,
  usageNote,
} from "./model";
import { MenuButton, type MenuItem } from "./ui/Menu";
import { Choice, Picker } from "./ui/Picker";

/** What a running session uses: one button that names the model and effort, with a menu that
 * changes them and the agent's own options, each a submenu, and the permission mode beside it.
 * The labels only move once the agent has changed, so they always say what's running. They
 * have no keys of their own besides the mode's: Tab from the message box reaches them. */
export function SessionSettings({
  machineId,
  session,
  provider,
  options,
  online,
  usage,
  input,
  onError,
}: {
  machineId: string;
  session: Session;
  /** Only what the agent reported can be offered, and a session may not have one. */
  provider: Provider | undefined;
  /** The agent's own options, like Claude's ultracode, which only a started agent reports. */
  options: AgentOption[];
  online: boolean;
  usage: { percent: number; used: number; size: number; cost?: string } | null;
  /** Where the keyboard goes back to, since the next thing to do is write. */
  input: RefObject<HTMLElement | null>;
  onError: (message: string | null) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [pickingMode, setPickingMode] = useState(false);
  const usable = online && !busy;

  if (!provider) return null;

  const model = provider.models.find((m) => m.id === session.model);
  const efforts = model?.efforts ?? [];
  const effort = efforts.find((e) => e.id === session.effort);
  const modes = MODES.filter((option) => provider.modes.includes(option.mode));
  const mode = MODES.find((option) => option.mode === session.mode);

  const run = async (command: Command) => {
    setBusy(true);
    onError(null);
    try {
      await request(machineId, command);
    } catch (reason) {
      onError(errorText(reason));
    } finally {
      setBusy(false);
    }
  };

  // Sent whole, so the menu waits for one change to finish before allowing the next.
  const change = (settings: { mode?: PermissionMode; model?: string | null; effort?: string | null }) =>
    run({
      command: "set_session_settings",
      session_id: session.id,
      mode: session.mode,
      model: session.model,
      effort: session.effort ?? null,
      ...settings,
    });

  const items: MenuItem[] = [];
  if (provider.models.length > 0) {
    items.push({
      id: "model",
      label: "Model",
      value: model?.name ?? session.model ?? "Default",
      run: () => {},
      items: provider.models.map((m) => ({
        id: m.id,
        label: modelLabel(provider, m),
        checked: m.id === session.model,
        run: () =>
          void change({ model: m.id, effort: effortForModel(provider, m, session.effort) }),
      })),
    });
  }
  if (efforts.length > 0) {
    items.push({
      id: "effort",
      label: "Effort",
      value: effort?.name ?? "Default",
      run: () => {},
      items: efforts.map((e) => ({
        id: e.id,
        label: e.name,
        checked: e.id === session.effort,
        run: () => void change({ effort: e.id }),
      })),
    });
  }
  for (const option of options) {
    const current = option.choices.find((choice) => choice.value === option.value);
    items.push({
      id: `option:${option.id}`,
      label: option.name,
      value: current?.name ?? option.value,
      run: () => {},
      items: option.choices.map((choice) => ({
        id: choice.value,
        label: choice.name,
        checked: choice.value === option.value,
        run: () =>
          void run({
            command: "set_session_option",
            session_id: session.id,
            option_id: option.id,
            value: choice.value,
          }),
      })),
    });
  }

  const label = settingsLabel({
    model: model?.name ?? session.model ?? "Default model",
    effort: effort?.name,
    changed: options.map(changedOption),
  });
  return (
    <div className="composer-pickers">
      <MenuButton
        label="Session settings"
        title="Model, effort and the agent's options"
        className="picker"
        align="start"
        groups={items.length > 0 ? [items] : []}
        disabled={!usable}
        note={usage ? usageNote(usage) : undefined}
        returnFocus={input}
      >
        {label}
      </MenuButton>
      {/* Apart, since it's what changes most, and has a key to go round. */}
      {modes.length > 0 && (
        <Picker
          className="picker"
          label="Permission mode"
          title={titled("Permission mode", "composer.cycle_mode")}
          value={session.mode}
          disabled={!usable}
          open={pickingMode}
          onOpenChange={setPickingMode}
          finalFocus={input}
          onChange={(value) => {
            const chosen = modes.find((option) => option.mode === value);
            if (chosen) void change({ mode: chosen.mode });
          }}
          trigger={mode?.label ?? "Unknown mode"}
        >
          {modes.map((option) => (
            <Choice key={option.mode} value={option.mode} label={option.label} />
          ))}
        </Picker>
      )}
    </div>
  );
}
