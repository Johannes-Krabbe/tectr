// Pure helpers that turn engine state into what the UI shows.

import type {
  ArchiveRisk,
  DaemonState,
  Device,
  DeviceSettings,
  MoveTarget,
  PermissionMode,
  Provider,
  ProviderEffort,
  ProviderModel,
  Session,
  SessionEvent,
  Terminal,
  ToolCallPatch,
  Topic,
  Workspace,
  Worktree,
} from "./engine";

/** `done` is finished and unread, `read` is finished and seen. */
export type StatusState = "working" | "waiting" | "failed" | "done" | "read" | "idle" | "stale";

export const STATE_LABEL: Record<StatusState, string> = {
  working: "Working",
  waiting: "Waiting",
  failed: "Failed",
  done: "Unread",
  read: "Done",
  idle: "Idle",
  stale: "Offline",
};

export const MODES: { mode: PermissionMode; label: string }[] = [
  { mode: "auto", label: "Auto" },
  { mode: "manual", label: "Manual" },
  { mode: "yolo", label: "Yolo" },
];

export function isOnline(device: Device): boolean {
  return device.connection.state === "online";
}

/** On unless someone turned it off, because the daemon leaves defaults out. */
export function keepsAwake(state: DaemonState): boolean {
  return state.settings?.keep_awake ?? true;
}

/** The device's settings with keeping awake flipped and everything else as it was. */
export function withKeepAwakeToggled(state: DaemonState): DeviceSettings {
  return { ...state.settings, keep_awake: !keepsAwake(state) };
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
      return session.unread ? "done" : "read";
    // An agent that stopped after finishing still has something to read.
    case "stopped":
      return session.unread ? "done" : "idle";
    default:
      return "idle";
  }
}

/** Something to read outranks an agent still working, which needs nothing from you. */
const URGENCY: StatusState[] = ["waiting", "failed", "done", "working", "read", "idle", "stale"];

/** A topic shows its most urgent session. See design/spec/components.md. */
export function topicState(topic: Topic, online: boolean): StatusState {
  if (!online) return "stale";
  const states = topic.sessions.map((session) => sessionState(session, online));
  return URGENCY.find((state) => states.includes(state)) ?? "idle";
}

/** What marking a topic read or unread changes: its unread sessions become read, or if it
 * has none, the ones that finished become unread again. */
export function unreadToggle(topic: Topic): { unread: boolean; sessions: Session[] } {
  const unread = topic.sessions.filter((session) => session.unread);
  if (unread.length) return { unread: false, sessions: unread };
  return { unread: true, sessions: topic.sessions.filter((session) => session.status === "done") };
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

export function terminalLabel(terminal: Terminal): string {
  return terminal.title || "Terminal";
}

/** One of a topic's tabs. */
export type Tab =
  | { kind: "session"; id: string; session: Session }
  | { kind: "terminal"; id: string; terminal: Terminal };

/** A topic's tabs in the order they were opened, agents and terminals together. Tabs opened
 * at the same moment keep the order the daemon lists them in. */
export function tabsOf(topic: Topic): Tab[] {
  const tabs = [
    ...topic.sessions.map((session): { tab: Tab; at: number } => ({
      tab: { kind: "session", id: session.id, session },
      at: session.opened_at_ms ?? 0,
    })),
    ...(topic.terminals ?? []).map((terminal): { tab: Tab; at: number } => ({
      tab: { kind: "terminal", id: terminal.id, terminal },
      at: terminal.opened_at_ms,
    })),
  ];
  return tabs.sort((a, b) => a.at - b.at).map(({ tab }) => tab);
}

export function tabLabel(tab: Tab): string {
  return tab.kind === "session" ? sessionLabel(tab.session) : terminalLabel(tab.terminal);
}

/** The folder a topic's tabs work in. */
export function topicWorktree(topic: Topic): Worktree | undefined {
  return topic.worktrees.find((worktree) => worktree.id === topic.worktree_id);
}

const counted = (count: number, thing: string) => `${count} ${thing}${count === 1 ? "" : "s"}`;

/** What archiving would throw away in git, one line each. Running agents aren't in it, because
 * their tabs come back with the topic. */
export function archiveLosses(risk: ArchiveRisk): string[] {
  return risk.worktrees.flatMap((worktree) => [
    ...(worktree.uncommitted
      ? [`${counted(worktree.uncommitted, "uncommitted file")} in ${worktree.path}`]
      : []),
    ...(worktree.unpushed
      ? [`${counted(worktree.unpushed, "commit")} on ${worktree.branch} that no other branch or remote has`]
      : []),
  ]);
}

/** The first thing archiving asks, which is all it asks when nothing in git would be lost. */
export function archiveSummary(risk: ArchiveRisk): string {
  const stops = risk.running ? `Stops ${counted(risk.running, "running agent")}. ` : "";
  return archiveLosses(risk).length
    ? `${stops}Its worktree has work that would be lost.`
    : `${stops}Its tabs come back when it's restored.`;
}

export interface MoveChoice {
  id: string;
  label: string;
  detail: string;
  target: MoveTarget;
}

/** Where a topic can move: a fresh worktree from where it is, the repo, or a worktree it left. */
export function moveChoices(topic: Topic, repoPath: string): MoveChoice[] {
  const here = topicWorktree(topic);
  const choices: MoveChoice[] = [
    {
      id: "new",
      label: "A new worktree",
      detail: here ? `From ${here.branch}` : "",
      target: { kind: "new_worktree" },
    },
  ];
  if (here?.path !== repoPath) {
    choices.push({ id: "root", label: "The repo", detail: repoPath, target: { kind: "root" } });
  }
  for (const worktree of topic.worktrees) {
    if (worktree === here || worktree.path === repoPath) continue;
    choices.push({
      id: worktree.id,
      label: worktree.branch,
      detail: worktree.path,
      target: { kind: "worktree", worktree_id: worktree.id },
    });
  }
  return choices;
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

/** Which activities show their steps, against what the setting says. `turn` is a whole turn
 * unfolded or folded from its live line, which also covers activities that haven't started
 * yet, so you can watch an agent work across its messages. `flipped` is single activities
 * pressed by hand, which win over both. */
export interface Folds {
  flipped: ReadonlySet<number>;
  turn: { prompt: number; open: boolean } | null;
}

export const NO_FOLDS: Folds = { flipped: new Set(), turn: null };

export function foldState(items: TranscriptItem[], folds: Folds, stepsShown: boolean) {
  // Each activity's turn is the prompt before it, and 0 before the first one.
  const promptOf = new Map<number, number>();
  let current = 0;
  for (const item of items) {
    if (item.kind === "prompt") current = item.key;
    else if (item.kind === "activity") promptOf.set(item.key, current);
  }
  const { turn, flipped } = folds;
  const turnOpen = turn?.prompt === current ? turn.open : stepsShown;
  const isOpen = (key: number) =>
    (turn && promptOf.get(key) === turn.prompt ? turn.open : stepsShown) !== flipped.has(key);
  return {
    isOpen,
    /** Whether the turn still going, or the last one, is unfolded. */
    turnOpen,
    flip: (key: number): Folds => {
      const next = new Set(flipped);
      if (!next.delete(key)) next.add(key);
      return { turn, flipped: next };
    },
    // Only one turn is held at a time, so the one it replaces keeps how it looks as flips.
    toggleTurn: (): Folds => ({
      turn: { prompt: current, open: !turnOpen },
      flipped: new Set(
        [...promptOf]
          .filter(([key, prompt]) => prompt !== current && isOpen(key) !== stepsShown)
          .map(([key]) => key),
      ),
    }),
  };
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
