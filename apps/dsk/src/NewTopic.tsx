import { Select } from "@base-ui/react/select";
import { type AgentChoice, type AgentDraft, AgentForm, type ExtraPicker } from "./AgentForm";
import { type Device, errorText, request, type Topic } from "./engine";
import { isOnline } from "./model";
import { Choice, Picker } from "./ui/Picker";
import { Keys } from "./ui/StatusIcon";

type Place = "worktree" | "root";

const PLACE_LABEL: Record<Place, string> = {
  worktree: "New worktree",
  root: "Repo root",
};

const PLACES: Place[] = ["root", "worktree"];

const EXTRA_PICKERS: ExtraPicker[] = [
  { name: "workspace", label: "Workspace", action: "new_topic.pick_workspace" },
  { name: "place", label: "Where to work", action: "new_topic.pick_place" },
];

/** The new agent's draft, plus where the topic goes. */
export interface NewTopicForm extends AgentDraft {
  at_root?: boolean;
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
  const choose = (change: NewTopicForm) => onForm({ ...form, ...change });
  const place: Place = (form.at_root ?? true) ? "root" : "worktree";

  const device = devices.find((d) => d.state?.machine.id === machineId);
  const workspace = device?.state?.workspaces.find((w) => w.id === workspaceId);
  const choices = devices.flatMap((d) =>
    (d.state?.workspaces ?? []).map((w) => ({
      value: placeOf(d.state!.machine.id, w.id),
      machineId: d.state!.machine.id,
      workspaceId: w.id,
    })),
  );

  const start = async (choice: AgentChoice) => {
    const created = await request(machineId, {
      command: "create_topic",
      workspace_id: workspaceId,
      name: null,
      at_root: place === "root",
    });
    if (created.output !== "topic") throw new Error("The daemon didn't create a topic");
    const topic: Topic = created.topic;
    try {
      const started = await request(machineId, {
        command: "start_session",
        topic_id: topic.id,
        agent: choice.agent,
        mode: choice.mode,
        model: choice.model,
        effort: choice.effort,
      });
      if (started.output !== "session") throw new Error("The daemon didn't start an agent");
      await request(machineId, {
        command: "send_prompt",
        session_id: started.session.id,
        text: choice.text,
        images: choice.images,
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
      <AgentForm
        device={device}
        draft={form}
        onDraft={choose}
        ready={workspace !== undefined}
        autoFocus
        extraPickers={EXTRA_PICKERS}
        onStart={start}
        onCancel={onCancel}
        onSettings={onSettings}
        selects={(control) => (
          <div className="create-selects">
            <Picker
              {...control("workspace")}
              label="Workspace"
              className="button"
              value={placeOf(machineId, workspaceId)}
              onChange={(value) => {
                const chosen = choices.find((c) => c.value === value);
                if (chosen) onWorkspace(chosen.machineId, chosen.workspaceId);
              }}
              trigger={
                <>
                  <span>{workspace?.name ?? "Choose a workspace"}</span>
                  {device?.state && (
                    <span className="create-device">{device.state.machine.name}</span>
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
              {...control("place")}
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
        )}
      />
    </>
  );
}
