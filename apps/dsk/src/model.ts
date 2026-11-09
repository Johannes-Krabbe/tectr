// Pure helpers that turn engine state into what the UI shows.

import type {
  Device,
  Provider,
  ProviderEffort,
  ProviderModel,
  Session,
  SessionEvent,
  ToolCallPatch,
  Topic,
  Workspace,
  Worktree,
} from "./engine";

export type StatusState = "working" | "waiting" | "failed" | "done" | "idle" | "stale";

export const STATE_LABEL: Record<StatusState, string> = {
  working: "Working",
  waiting: "Waiting",
  failed: "Failed",
  done: "Done",
  idle: "Idle",
  stale: "Offline",
};

export function isOnline(device: Device): boolean {
  return device.connection.state === "online";
}

/** Only the agent's own recommendation is marked. */
export function modelLabel(provider: Provider, model: ProviderModel): string {
  return model.id === provider.recommended_model ? `${model.name} (recommended)` : model.name;
}

/** The provider's default effort, or a middle level if this model doesn't have it. */
export function defaultEffort(
  provider: Provider,
  model: ProviderModel,
): ProviderEffort | undefined {
  return (
    model.efforts.find((effort) => effort.id === provider.default_effort) ??
    model.efforts[Math.floor(model.efforts.length / 2)]
  );
}

/** The effort to use after switching to `model`: the current one if it's on offer there. */
export function effortForModel(
  provider: Provider,
  model: ProviderModel,
  current: string | null | undefined,
): string | null {
  const kept = model.efforts.find((effort) => effort.id === current);
  return (kept ?? defaultEffort(provider, model))?.id ?? null;
}

export function sessionState(session: Session, online: boolean): StatusState {
  if (!online) return "stale";
  switch (session.status) {
    case "starting":
    case "working":
      return "working";
    case "waiting_for_you":
      return "waiting";
    case "failed":
      return "failed";
    case "done":
      return "done";
    default:
      return "idle";
  }
}

const URGENCY: StatusState[] = ["waiting", "failed", "working", "done", "idle", "stale"];

/** A topic shows its most urgent session. See design/spec/components.md. */
export function topicState(topic: Topic, online: boolean): StatusState {
  if (!online) return "stale";
  const states = topic.sessions.map((session) => sessionState(session, online));
  return URGENCY.find((state) => states.includes(state)) ?? "idle";
}

export interface TopicEntry {
  device: Device;
  machineId: string;
  workspace: Workspace;
  topic: Topic;
}

/** Every topic in sidebar order. */
export function allTopics(devices: Device[]): TopicEntry[] {
  return devices.flatMap((device) =>
    (device.state?.workspaces ?? []).flatMap((workspace) =>
      workspace.topics.map((topic) => ({
        device,
        machineId: device.state!.machine.id,
        workspace,
        topic,
      })),
    ),
  );
}

export function sessionLabel(session: Session): string {
  return session.title ?? session.agent;
}

/** The worktree a session runs in. A topic can have several, so its first isn't necessarily it. */
export function sessionWorktree(topic: Topic, session: Session): Worktree | undefined {
  return topic.worktrees.find((worktree) => worktree.id === session.worktree_id);
}

/** What deleting a topic does, so it can be said before it happens. The daemon never touches a
 * topic's work in git, and a topic at the repo root has no worktree of its own to remove. */
export function deleteTopicEffect(topic: Topic, repoPath: string): string {
  const agents = topic.sessions.length > 0 ? "Stops its agents. " : "";
  const atRoot = topic.worktrees.every((worktree) => worktree.path === repoPath);
  return (
    agents +
    (atRoot
      ? "The repo stays as it is."
      : "Removes its worktree, but keeps uncommitted work and unmerged commits.")
  );
}

export type ActivityStep =
  | { kind: "thought"; key: number; text: string }
  | { kind: "tool"; key: number; tool: ToolCallPatch }
  | { kind: "note"; key: number; text: string };

/** What an agent did between two things it said: its tool calls and thinking, as one line. */
export interface Activity {
  kind: "activity";
  key: number;
  steps: ActivityStep[];
  startedMs: number;
  endedMs: number;
  /** Time spent on a permission request, which is you and not the agent. */
  waitedMs: number;
}

export type TranscriptItem =
  | { kind: "prompt" | "message"; key: number; text: string }
  | Activity
  | { kind: "note"; key: number; text: string; failed?: boolean };

const STOP_NOTES: Record<string, string> = {
  cancelled: "Interrupted",
  max_tokens: "Stopped at the token limit",
  max_turn_requests: "Stopped at the turn limit",
  refusal: "The agent refused",
};

/** Folds a session's events into what a person reads: streamed chunks become one message,
 * and everything an agent does between messages becomes one activity. */
export function foldTranscript(events: SessionEvent[]): TranscriptItem[] {
  const items: TranscriptItem[] = [];
  const tools = new Map<string, Extract<ActivityStep, { kind: "tool" }>>();
  const requests = new Map<string, { title: string; options: Map<string, string> }>();
  let open: Activity | null = null;
  let previousAt: number | null = null;
  let requestsOpen = 0;
  let waitingSince = 0;

  const activity = (seq: number, at: number): Activity => {
    if (!open) {
      // It began when the agent went quiet, not when its first step showed up.
      open = {
        kind: "activity",
        key: seq,
        steps: [],
        startedMs: previousAt ?? at,
        endedMs: at,
        waitedMs: 0,
      };
      items.push(open);
    }
    open.endedMs = at;
    return open;
  };

  const touch = (at: number) => {
    if (open) open.endedMs = at;
  };

  const close = (at: number) => {
    touch(at);
    open = null;
  };

  for (const { seq, at_ms: at, kind } of events) {
    switch (kind.event) {
      case "user_prompt":
        close(at);
        items.push({ kind: "prompt", key: seq, text: kind.text });
        break;
      case "agent_message": {
        close(at);
        const last = items[items.length - 1];
        if (last?.kind === "message") last.text += kind.text;
        else items.push({ kind: "message", key: seq, text: kind.text });
        break;
      }
      case "agent_thought": {
        const { steps } = activity(seq, at);
        const last = steps[steps.length - 1];
        if (last?.kind === "thought") last.text += kind.text;
        else steps.push({ kind: "thought", key: seq, text: kind.text });
        break;
      }
      case "tool_call":
      case "tool_call_update": {
        const known = tools.get(kind.id);
        if (known) {
          known.tool = { ...known.tool, ...kind.tool };
          // Tools can finish after a message, and that doesn't reopen their activity.
          touch(at);
        } else {
          const step = { kind: "tool" as const, key: seq, tool: kind.tool };
          tools.set(kind.id, step);
          activity(seq, at).steps.push(step);
        }
        break;
      }
      case "permission_requested":
        requests.set(kind.approval.id, {
          title: kind.approval.tool_call.title ?? "Untitled tool call",
          options: new Map(kind.approval.options.map((option) => [option.id, option.name])),
        });
        activity(seq, at);
        if (requestsOpen++ === 0) waitingSince = at;
        break;
      case "permission_resolved": {
        const request = requests.get(kind.approval_id);
        const answer =
          kind.outcome.outcome === "selected"
            ? (request?.options.get(kind.outcome.option_id) ?? "Answered")
            : "Cancelled";
        const current = activity(seq, at);
        if (--requestsOpen === 0) current.waitedMs += at - waitingSince;
        current.steps.push({
          kind: "note",
          key: seq,
          text: `Request: ${request?.title ?? "tool call"}. ${answer}`,
        });
        break;
      }
      case "agent_ready":
        close(at);
        items.push({
          kind: "note",
          key: seq,
          text: [kind.name, kind.version, kind.current_mode && `in mode ${kind.current_mode}`]
            .filter(Boolean)
            .join(" "),
        });
        break;
      case "turn_ended": {
        close(at);
        const note = STOP_NOTES[kind.stop_reason];
        if (note) items.push({ kind: "note", key: seq, text: note });
        break;
      }
      case "error":
        close(at);
        items.push({ kind: "note", key: seq, text: kind.message, failed: true });
        break;
      default:
        break;
    }
    previousAt = at;
  }
  return items;
}

/** What in a transcript is happening now, as opposed to what it already had when it opened.
 * History is replayed an event at a time across many updates, so nothing about how events
 * arrive tells the two apart. Only when they happened does. */
export function transcriptTiming(events: SessionEvent[], openedMs: number) {
  const last = events[events.length - 1];
  return {
    /** An event from after opening has come, so history is done replaying. */
    streaming: last !== undefined && last.at_ms >= openedMs,
    /** Whether this item began after opening. Seqs start at 1 and have no gaps. */
    isFresh: (key: number) => (events[key - 1]?.at_ms ?? 0) >= openedMs,
  };
}

/** A length of time in the app's short form, never less than a second. */
export function formatWorked(ms: number): string {
  const seconds = Math.max(1, Math.round(ms / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${seconds % 60}s`;
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}

/** The line a finished activity folds into. */
export function activitySummary(activity: Activity): { text: string; failed: number } {
  const worked = formatWorked(activity.endedMs - activity.startedMs - activity.waitedMs);
  const counted = activity.steps.filter((step) => step.kind !== "note");
  const failed = counted.filter(
    (step) => step.kind === "tool" && step.tool.status === "failed",
  ).length;
  if (counted.length > 0 && counted.every((step) => step.kind === "thought")) {
    return { text: `Thought for ${worked}`, failed };
  }
  const count = counted.length === 1 ? "1 step" : `${counted.length} steps`;
  return { text: `Worked for ${worked}, ${count}`, failed };
}

/** What a running session is on right now, for the line at the end of its transcript.
 * Parallel tools show the newest one still running, and a step stays until the next one
 * replaces it, so a run of quick tools never flickers back to a generic word. */
export function liveStepLabel(session: Session, activity: Activity | null): string {
  if (session.status === "waiting_for_you") return "Waiting for you";
  if (session.status === "starting") return "Starting";
  const steps = (activity?.steps ?? []).filter((step) => step.kind !== "note");
  const running = steps.filter(
    (step) =>
      step.kind !== "tool" || (step.tool.status !== "completed" && step.tool.status !== "failed"),
  );
  const step = running[running.length - 1] ?? steps[steps.length - 1];
  if (!step) return "Working";
  return step.kind === "tool" ? (step.tool.title ?? "Tool call") : "Thinking";
}

/** A time in the past, in the app's short form. */
export function ago(ms: number): string {
  const minutes = Math.floor((Date.now() - ms) / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  return hours < 24 ? `${hours}h ago` : `${Math.floor(hours / 24)}d ago`;
}
