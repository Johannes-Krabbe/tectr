import { type RefObject, useEffect, useRef, useState } from "react";
import {
  errorText,
  type PermissionMode,
  type Provider,
  request,
  type Session,
} from "./engine";
import { act, type ActionId, titled } from "./keybinds";
import { inSidebar } from "./keys";
import { effortForModel, modelLabel, MODES } from "./model";
import { hint, type Hint } from "./shortcuts";
import { Choice, Picker } from "./ui/Picker";

type PickerName = "model" | "effort" | "mode";

const PICKER_KEYS: [ActionId, PickerName][] = [
  ["pane.pick_model", "model"],
  ["pane.pick_effort", "effort"],
  ["pane.pick_mode", "mode"],
];

/** The pickers `SessionPickers` has for this session, for the shortcut line. */
export function sessionPickerHints(
  provider: Provider | undefined,
  session: Session,
  online: boolean,
): Hint[] {
  if (!provider || !online) return [];
  const model = provider.models.find((m) => m.id === session.model);
  const hints: Hint[] = [];
  if (provider.models.length > 0) hints.push(hint("Model", "pane.pick_model"));
  if (model?.efforts.length) hints.push(hint("Effort", "pane.pick_effort"));
  if (MODES.some((option) => provider.modes.includes(option.mode))) {
    hints.push(hint("Permission mode", "pane.pick_mode"));
  }
  return hints;
}

/** What a running session uses, which can change at any time. The labels only move once the
 * agent has changed, so they always say what's running. */
export function SessionPickers({
  machineId,
  session,
  provider,
  online,
  input,
  transcript,
  onError,
}: {
  machineId: string;
  session: Session;
  /** Only what the agent reported can be offered, and a session may not have one. */
  provider: Provider | undefined;
  online: boolean;
  input: RefObject<HTMLElement | null>;
  transcript: RefObject<HTMLElement | null>;
  onError: (message: string | null) => void;
}) {
  const [picking, setPicking] = useState<PickerName | null>(null);
  const [busy, setBusy] = useState(false);
  // Choosing by key goes back to whichever had the keyboard, and by pointer to the message box.
  const returnFocus = useRef<HTMLElement | null>(null);
  const usable = provider !== undefined && online && !busy;
  const live = useRef(usable);
  live.current = usable;

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (inSidebar() || document.querySelector(".sheet")) return;
      PICKER_KEYS.some(([id, picker]) =>
        act(event, id, () => {
          if (!live.current) return;
          returnFocus.current =
            document.activeElement === input.current ? input.current : transcript.current;
          setPicking((shown) => (shown === picker ? null : picker));
        }),
      );
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

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
          title={titled("Model", "pane.pick_model")}
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
          title={titled("Effort", "pane.pick_effort")}
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
          title={titled("Permission mode", "pane.pick_mode")}
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
