import { type RefObject, useRef, useState } from "react";
import {
  errorText,
  type PermissionMode,
  type Provider,
  request,
  type Session,
} from "./engine";
import { effortForModel, modelLabel, MODES } from "./model";
import { Choice, Picker } from "./ui/Picker";

type PickerName = "model" | "effort" | "mode";

/** What a running session uses, which can change at any time. The labels only move once the
 * agent has changed, so they always say what's running. They have no keys of their own: Tab
 * from the message box reaches them. */
export function SessionPickers({
  machineId,
  session,
  provider,
  online,
  input,
  onError,
}: {
  machineId: string;
  session: Session;
  /** Only what the agent reported can be offered, and a session may not have one. */
  provider: Provider | undefined;
  online: boolean;
  input: RefObject<HTMLElement | null>;
  onError: (message: string | null) => void;
}) {
  const [picking, setPicking] = useState<PickerName | null>(null);
  const [busy, setBusy] = useState(false);
  // Choosing goes back to the message box, which is where the next thing to do is.
  const returnFocus = useRef<HTMLElement | null>(null);
  const usable = provider !== undefined && online && !busy;

  if (!provider) return null;

  const model = provider.models.find((m) => m.id === session.model);
  const efforts = model?.efforts ?? [];
  const effort = efforts.find((e) => e.id === session.effort);
  const modes = MODES.filter((option) => provider.modes.includes(option.mode));
  const mode = MODES.find((option) => option.mode === session.mode);

  // Sent whole, so the pickers wait for one change to finish before allowing the next.
  const change = async (settings: {
    mode?: PermissionMode;
    model?: string | null;
    effort?: string | null;
  }) => {
    setBusy(true);
    onError(null);
    try {
      await request(machineId, {
        command: "set_session_settings",
        session_id: session.id,
        mode: session.mode,
        model: session.model,
        effort: session.effort ?? null,
        ...settings,
      });
    } catch (reason) {
      onError(errorText(reason));
    } finally {
      setBusy(false);
    }
  };

  const pickerProps = (name: PickerName) => ({
    className: "picker",
    disabled: !usable,
    finalFocus: returnFocus,
    open: picking === name,
    onOpenChange: (open: boolean) => {
      if (open) returnFocus.current = input.current;
      setPicking(open ? name : null);
    },
  });

  return (
    <div className="composer-pickers">
      {provider.models.length > 0 && (
        <Picker
          {...pickerProps("model")}
          label="Model"
          title="Model"
          value={session.model}
          onChange={(id) => {
            const next = provider.models.find((m) => m.id === id);
            if (next) {
              void change({ model: id, effort: effortForModel(provider, next, session.effort) });
            }
          }}
          trigger={model?.name ?? session.model ?? "Default model"}
        >
          {provider.models.map((m) => (
            <Choice key={m.id} value={m.id} label={modelLabel(provider, m)} />
          ))}
        </Picker>
      )}
      {efforts.length > 0 && (
        <Picker
          {...pickerProps("effort")}
          label="Effort"
          title="Effort"
          value={session.effort ?? null}
          onChange={(id) => void change({ effort: id })}
          trigger={effort ? `${effort.name} effort` : "Default effort"}
        >
          {efforts.map((e) => (
            <Choice key={e.id} value={e.id} label={e.name} />
          ))}
        </Picker>
      )}
      {modes.length > 0 && (
        <Picker
          {...pickerProps("mode")}
          label="Permission mode"
          title="Permission mode"
          value={session.mode}
          onChange={(value) => void change({ mode: value as PermissionMode })}
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
