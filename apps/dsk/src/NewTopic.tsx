import { Select } from "@base-ui/react/select";
import { useEffect, useRef, useState } from "react";
import {
  type Device,
  errorText,
  type PermissionMode,
  request,
  type Topic,
} from "./engine";
import { act, type ActionId, textBoxKey, titled } from "./keybinds";
import { defaultEffort, isOnline, modelLabel } from "./model";
import { hint, type Hint, useHints } from "./shortcuts";
import { MODES } from "./Sheets";
import { Choice, Picker } from "./ui/Picker";
import { Keys } from "./ui/StatusIcon";

type Place = "worktree" | "root";

const PLACE_LABEL: Record<Place, string> = {
  worktree: "New worktree",
  root: "Repo root",
};

const PLACES: Place[] = ["root", "worktree"];

type PickerName = "workspace" | "place" | "provider" | "model" | "effort" | "mode";

/** These open a picker, from the chatbox, which keeps the keyboard the whole time. */
const PICKER_KEYS: [ActionId, PickerName][] = [
  ["new_topic.pick_workspace", "workspace"],
  ["new_topic.pick_place", "place"],
  ["new_topic.pick_provider", "provider"],
  ["new_topic.pick_model", "model"],
  ["new_topic.pick_effort", "effort"],
  ["new_topic.pick_mode", "mode"],
];

/** What's been chosen and typed so far. Only what was changed is here: the rest comes from the
 * provider's defaults, and a choice that doesn't apply to the current device or model is
 * ignored. It belongs to this client and isn't saved anywhere, so restarting clears it. */
export interface NewTopicForm {
  text?: string;
  at_root?: boolean;
  agent?: string;
  model?: string;
  effort?: string;
  mode?: PermissionMode;
}

const placeOf = (machineId: string, workspaceId: string) => `${machineId}/${workspaceId}`;

/** Starts a topic anywhere: pick a workspace, say where to work, and type the first prompt.
 * The workspace comes from the view, so whatever opened this screen can preset it. */
export function NewTopic({
  devices,
  machineId,
  workspaceId,
  form,
  onForm,
  onWorkspace,
  onSettings,
  onCancel,
  onStarted,
  onStalled,
}: {
  devices: Device[];
  machineId: string;
  workspaceId: string;
  form: NewTopicForm;
  onForm: (form: NewTopicForm) => void;
  onWorkspace: (machineId: string, workspaceId: string) => void;
  onSettings: (machineId: string) => void;
  onCancel: () => void;
  onStarted: (machineId: string, topicId: string, sessionId: string) => void;
  /** The topic exists but its agent didn't start, so retrying here would make a second one. */
  onStalled: (machineId: string, topicId: string, reason: string) => void;
}) {
  const text = form.text ?? "";
  const choose = (change: NewTopicForm) => onForm({ ...form, ...change });
  const place: Place = (form.at_root ?? true) ? "root" : "worktree";
  const [picking, setPicking] = useState<PickerName | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const input = useRef<HTMLTextAreaElement>(null);

  const device = devices.find((d) => d.state?.machine.id === machineId);
  const workspace = device?.state?.workspaces.find((w) => w.id === workspaceId);
  const online = device !== undefined && isOnline(device);
  const choices = devices.flatMap((d) =>
    (d.state?.workspaces ?? []).map((w) => ({
      value: placeOf(d.state!.machine.id, w.id),
      machineId: d.state!.machine.id,
      workspaceId: w.id,
    })),
  );

  const providers = device?.state?.providers ?? [];
  const provider = providers.find((p) => p.agent === form.agent) ?? providers[0];
  const model =
    provider?.models.find((m) => m.id === form.model) ??
    provider?.models.find((m) => m.id === provider.default_model) ??
    provider?.models[0];
  const efforts = model?.efforts ?? [];
  const effort =
    efforts.find((e) => e.id === form.effort) ??
    (provider && model ? defaultEffort(provider, model) : undefined);
  const modes = MODES.filter((option) => provider?.modes.includes(option.mode));
  const mode =
    modes.find((option) => option.mode === form.mode) ??
    modes.find((option) => option.mode === "auto") ??
    modes[0];

  const canStart =
    workspace !== undefined &&
    online &&
    provider !== undefined &&
    mode !== undefined &&
    text.trim() !== "" &&
    !busy;

  const pickerProps = (name: PickerName) => ({
    finalFocus: input,
    open: picking === name,
    onOpenChange: (open: boolean) => setPicking(open ? name : null),
  });

  const pickerHints: Hint[] = [
    hint("Workspace", "new_topic.pick_workspace"),
    hint("Where to work", "new_topic.pick_place"),
  ];
  if (provider) pickerHints.push(hint("Provider", "new_topic.pick_provider"));
  if (model) pickerHints.push(hint("Model", "new_topic.pick_model"));
  if (efforts.length > 0) pickerHints.push(hint("Effort", "new_topic.pick_effort"));
  if (mode) pickerHints.push(hint("Permission mode", "new_topic.pick_mode"));
  useHints("pane", [...pickerHints, hint("Sidebar", "pane.back")]);
  useHints("typing", [
    ...(canStart ? [hint("Start", "new_topic.start")] : []),
    hint("New line", "new_topic.newline"),
    hint("Cancel", "new_topic.cancel"),
    ...pickerHints,
  ]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (document.querySelector(".sheet")) return;
      PICKER_KEYS.some(([id, picker]) =>
        act(event, id, () => setPicking((shown) => (shown === picker ? null : picker))),
      );
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const start = async () => {
    if (!canStart) return;
    setBusy(true);
    setError(null);
    let topic: Topic;
    try {
      const created = await request(machineId, {
        command: "create_topic",
        workspace_id: workspaceId,
        name: null,
        at_root: place === "root",
      });
      if (created.output !== "topic") throw new Error("The daemon didn't create a topic");
      topic = created.topic;
    } catch (reason) {
      setError(errorText(reason));
      setBusy(false);
      return;
    }
    try {
      const started = await request(machineId, {
        command: "start_session",
        topic_id: topic.id,
        worktree_id: topic.worktrees[0].id,
        agent: provider.agent,
        mode: mode.mode,
        model: model?.id ?? null,
        effort: effort?.id ?? null,
      });
      if (started.output !== "session") throw new Error("The daemon didn't start an agent");
      await request(machineId, {
        command: "send_prompt",
        session_id: started.session.id,
        text: text.trim(),
      });
      onStarted(machineId, topic.id, started.session.id);
    } catch (reason) {
      onStalled(machineId, topic.id, errorText(reason));
    }
  };

  return (
    <>
      <header className="head">
        <div className="crumb">New topic</div>
      </header>
      <form
        className="pane new-topic"
        onSubmit={(event) => {
          event.preventDefault();
          void start();
        }}
      >
        <div className="new-topic-in">
          <div className="new-topic-selects">
            <Picker
              {...pickerProps("workspace")}
              label="Workspace"
              className="button"
              value={placeOf(machineId, workspaceId)}
              onChange={(value) => {
                const choice = choices.find((c) => c.value === value);
                if (choice) onWorkspace(choice.machineId, choice.workspaceId);
              }}
              trigger={
                <>
                  <span>{workspace?.name ?? "Choose a workspace"}</span>
                  {device?.state && (
                    <span className="new-topic-device">{device.state.machine.name}</span>
                  )}
                  <Keys action="new_topic.pick_workspace" />
                </>
              }
            >
              {devices
                .filter((d) => d.state?.workspaces.length)
                .map((d) => (
                  <Select.Group key={d.url}>
                    <Select.GroupLabel className="select-device">
                      {d.state!.machine.name}
                    </Select.GroupLabel>
                    {d.state!.workspaces.map((w) => (
                      <Choice
                        key={w.id}
                        value={placeOf(d.state!.machine.id, w.id)}
                        label={w.name}
                        meta={isOnline(d) ? undefined : "Offline"}
                        disabled={!isOnline(d)}
                      />
                    ))}
                  </Select.Group>
                ))}
            </Picker>
            <Picker
              {...pickerProps("place")}
              label="Where to work"
              className="button"
              value={place}
              onChange={(value) => choose({ at_root: value === "root" })}
              trigger={
                <>
                  <span>{PLACE_LABEL[place]}</span>
                  <Keys action="new_topic.pick_place" />
                </>
              }
            >
              {PLACES.map((option) => (
                <Choice key={option} value={option} label={PLACE_LABEL[option]} />
              ))}
            </Picker>
          </div>
          <div className="composer">
            <textarea
              ref={input}
              autoFocus
              readOnly={busy}
              value={text}
              placeholder="What should the agent do?"
              aria-label="First prompt"
              autoCorrect="off"
              autoCapitalize="off"
              onChange={(event) => choose({ text: event.target.value })}
              onKeyDown={(event) =>
                textBoxKey(
                  event,
                  ["new_topic.start", () => void start()],
                  "new_topic.newline",
                  ["new_topic.cancel", onCancel],
                )
              }
            />
            <div className="composer-pickers">
              {provider ? (
                <>
                  <Picker
                    {...pickerProps("provider")}
                    label="Provider"
                    title={titled("Provider", "new_topic.pick_provider")}
                    className="picker"
                    value={provider.agent}
                    onChange={(agent) => choose({ agent })}
                    trigger={provider.name}
                  >
                    {providers.map((p) => (
                      <Choice key={p.agent} value={p.agent} label={p.name} />
                    ))}
                  </Picker>
                  {model && (
                    <Picker
                      {...pickerProps("model")}
                      label="Model"
                      title={titled("Model", "new_topic.pick_model")}
                      className="picker"
                      value={model.id}
                      onChange={(model) => choose({ model })}
                      trigger={modelLabel(provider, model)}
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
                      title={titled("Effort", "new_topic.pick_effort")}
                      className="picker"
                      value={effort?.id ?? null}
                      onChange={(effort) => choose({ effort })}
                      trigger={`${effort?.name} effort`}
                    >
                      {efforts.map((e) => (
                        <Choice key={e.id} value={e.id} label={e.name} />
                      ))}
                    </Picker>
                  )}
                  {mode && (
                    <Picker
                      {...pickerProps("mode")}
                      label="Permission mode"
                      title={titled("Permission mode", "new_topic.pick_mode")}
                      className="picker"
                      value={mode.mode}
                      onChange={(value) => choose({ mode: value as PermissionMode })}
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
                  disabled={!online}
                  onClick={() => onSettings(machineId)}
                >
                  Add a provider
                </button>
              )}
            </div>
          </div>
          <button className="button primary new-topic-start" type="submit" disabled={!canStart}>
            {busy ? "Starting" : "Start"}
            <Keys action="new_topic.start" />
          </button>
          {device?.state && !online && (
            <p className="error">{device.state.machine.name} is offline</p>
          )}
          {error && <p className="error">{error}</p>}
        </div>
      </form>
    </>
  );
}
