// Pure helpers that turn engine state into what the UI shows.

import type {
  AgentOption,
  ArchiveRisk,
  BackgroundTask,
  Conversation,
  DaemonState,
  Device,
  DeviceSettings,
  Diff,
  MoveTarget,
  PermissionMode,
  PlanEntry,
  Provider,
  ProviderEffort,
  ProviderModel,
  PromptImage,
  Question,
  QuestionAnswer,
  RepoClone,
  Session,
  SessionEvent,
  SubagentStatus,
  TaskStatus,
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
  { mode: "accept_edits", label: "Accept Edits" },
  { mode: "yolo", label: "Yolo" },
  { mode: "plan", label: "Plan" },
];

/** The mode after `current` among those an agent offers, going round. */
export function nextMode(offered: PermissionMode[], current: PermissionMode): PermissionMode | null {
  const modes = MODES.map((m) => m.mode).filter((mode) => offered.includes(mode));
  if (modes.length === 0) return null;
  return modes[(modes.indexOf(current) + 1) % modes.length];
}

/** The mode a new agent starts in unless another is chosen: Auto where the agent has it. */
export function defaultMode(offered: PermissionMode[]): PermissionMode | undefined {
  return offered.includes("auto") ? "auto" : MODES.find((m) => offered.includes(m.mode))?.mode;
}

/** An agent option's choice once it's turned on or away from its default, or null while it's
 * Off or Default. An on and off option reads as its own name, like "Ultracode", and any other
 * as the choice. Agents list choices in any order, so it goes by their names. */
export function changedOption(option: AgentOption): string | null {
  const resting = (name: string) => ["off", "default", "none"].includes(name.trim().toLowerCase());
  const current = option.choices.find((choice) => choice.value === option.value);
  const name = current?.name ?? option.value;
  if (resting(name)) return null;
  const toggle = option.choices.some((choice) => choice.name.trim().toLowerCase() === "off");
  return toggle ? option.name : name;
}

/** What a session runs on in one short line, like "Opus 5.5 high, Ultracode": the model and
 * effort always, and the agent's options only once they're on. */
export function settingsLabel(parts: {
  model: string;
  effort?: string;
  changed: (string | null)[];
}): string {
  const head = parts.effort ? `${parts.model} ${parts.effort.toLowerCase()}` : parts.model;
  return [head, ...parts.changed.filter((part) => part !== null)].join(", ");
}

/** How full the agent's context is and what it's cost, for the settings menu. */
export function usageNote(usage: { percent: number; used: number; size: number; cost?: string }) {
  const tokens = `${usage.percent}% of context, ${shortCount(usage.used)} of ${shortCount(usage.size)} tokens`;
  return usage.cost ? `${tokens}, ${usage.cost} so far` : tokens;
}

/** From here on the message box shows how full the context is, since a compaction is near. */
export const USAGE_SHOWN_FROM = 80;

/** Whether commands can be sent, which a device that's slow to answer still takes. */
export function isOnline(device: Device): boolean {
  return device.connection.state === "online";
}

/** What to call a device, which before its first connection is only its address. */
export function deviceName(device: Device): string {
  return device.state?.machine.name ?? device.url;
}

/** Why a device can't be reached, in the words of whatever refused. */
export function connectionProblem(device: Device): string | undefined {
  const { connection } = device;
  if (connection.state === "offline") return connection.reason;
  if (connection.state === "asleep") return "It's asleep, and shows up again once it wakes";
  if (connection.state === "incompatible") {
    return `The daemon speaks protocol ${connection.daemon_min} to ${connection.daemon_max}, which this app doesn't`;
  }
  return undefined;
}

/** The home folder as `~`, which is how people read their own paths. */
export const homeShort = (path: string) => path.replace(/^\/(Users|home)\/[^/]+(?=\/|$)/, "~");

/** How a clone is going, in a few words: what git is doing and how far along, or that it
 * failed. Git's phases start with what they do, like "Receiving objects", so the first word is
 * enough. */
export function cloneProgress(clone: RepoClone): { text: string; failed: boolean } {
  if (clone.error) return { text: "Failed", failed: true };
  if (clone.phase === undefined || clone.percent === undefined) return { text: "Cloning", failed: false };
  return { text: `${clone.phase.split(" ")[0]} ${clone.percent}%`, failed: false };
}

/** What a device's row and page say beside its name, and whether it's something wrong. Nothing
 * while it's online with nothing to say. */
export function deviceStatus(device: Device, now: number): { text: string; alert?: boolean } | null {
  switch (device.connection.state) {
    case "online": {
      if (device.connection.stalled) return { text: "Not responding", alert: true };
      const until = awakeUntil(device.state, now);
      return until === null ? null : { text: `Awake, ${timeLeft(until - now)}` };
    }
    case "connecting":
      return { text: "Connecting" };
    case "incompatible":
      return { text: "Needs an update" };
    case "offline":
      return { text: device.last_seen_ms ? `Offline, ${ago(device.last_seen_ms)}` : "Offline" };
    case "asleep":
      return { text: device.last_seen_ms ? `Asleep, ${ago(device.last_seen_ms)}` : "Asleep" };
  }
}

/** What a pane says about its device while something sent there might not be answered. */
export interface DeviceNotice {
  text: string;
  /** Why, for a tooltip. */
  detail?: string;
  /** Whether trying again now could help. */
  reconnect: boolean;
}

/** The worst of what's keeping a device from answering, or null when nothing is. `waiting` is
 * whether a command has gone unanswered for long enough to mention. */
export function deviceNotice(device: Device, waiting: boolean): DeviceNotice | null {
  const name = deviceName(device);
  const detail = connectionProblem(device);
  switch (device.connection.state) {
    case "connecting":
      return { text: `Connecting to ${name}`, reconnect: false };
    case "offline":
      return { text: `${name} is offline`, detail, reconnect: true };
    case "asleep":
      return { text: `${name} is asleep`, detail, reconnect: true };
    case "incompatible":
      return { text: `${name} needs an update`, detail, reconnect: true };
    case "online":
      if (device.connection.stalled) return { text: `${name} isn't responding`, reconnect: false };
      return waiting ? { text: `Waiting for ${name}`, reconnect: false } : null;
  }
}

/** What the keep-awake box offers before anything is typed. */
export const KEEP_AWAKE_SUGGESTIONS = ["1h", "2h", "4h", "8h"];

/** The daemon's limit, so a typo is caught before it's sent. */
const MAX_KEEP_AWAKE_SECS = 7 * 24 * 60 * 60;

const UNIT_SECS: Record<string, number> = { d: 86_400, h: 3_600, m: 60 };

/** Seconds in a duration like "2h", "90m", "1h30m", "1.5h" or "1d", or a bare number of
 * hours, since that's what the suggestions are in. Null for anything else, nothing, or more
 * than the daemon takes. */
export function parseDuration(text: string): number | null {
  const typed = text.trim().toLowerCase().replace(/\s+/g, "");
  if (/^\d+(\.\d+)?$/.test(typed)) return withinLimit(Number(typed) * 3_600);
  if (!/^(\d+(\.\d+)?[dhm])+$/.test(typed)) return null;
  let seconds = 0;
  for (const [, amount, unit] of typed.matchAll(/(\d+(?:\.\d+)?)([dhm])/g)) {
    seconds += Number(amount) * UNIT_SECS[unit];
  }
  return withinLimit(seconds);
}

function withinLimit(seconds: number): number | null {
  const whole = Math.round(seconds);
  return whole > 0 && whole <= MAX_KEEP_AWAKE_SECS ? whole : null;
}

/** Until when the device stays awake because someone asked, or null when it doesn't. */
export function awakeUntil(state: DaemonState | null, now: number): number | null {
  const until = state?.awake_until_ms;
  return until !== undefined && until > now ? until : null;
}

/** "1h 40m" for what's left, rounded up so it never says nothing while time is left. */
export function timeLeft(ms: number): string {
  const minutes = Math.max(Math.ceil(ms / 60_000), 1);
  const days = Math.floor(minutes / 1_440);
  const hours = Math.floor((minutes % 1_440) / 60);
  const rest = minutes % 60;
  const parts = [days && `${days}d`, hours && `${hours}h`, rest && `${rest}m`];
  return parts.filter(Boolean).join(" ");
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

/** The most urgent state any of a workspace's topics is in, and how many are in it. */
export function workspaceState(
  workspace: Workspace,
  online: boolean,
): { state: StatusState; count: number } {
  const states = workspace.topics.map((topic) => topicState(topic, online));
  const state = URGENCY.find((s) => states.includes(s)) ?? "idle";
  return { state, count: states.filter((s) => s === state).length };
}

/** The agents closing a workspace would stop: starting, working or waiting. */
export function runningIn(workspace: Workspace): number {
  return workspace.topics
    .flatMap((topic) => topic.sessions)
    .filter((s) => s.status === "starting" || s.status === "working" || s.status === "waiting_for_you")
    .length;
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
  | { kind: "terminal"; id: string; terminal: Terminal }
  | { kind: "diff"; id: string; diff: Diff }
  | { kind: "task"; id: string; sessionId: string; task: BackgroundTask };

/** A topic's tabs in the order they were opened, whatever their kind. Tabs opened at the same
 * moment keep the order the daemon lists them in. A session's background tasks come right
 * after it, since its agent opened them. */
export function tabsOf(topic: Topic, tasks: Record<string, BackgroundTask[]> = {}): Tab[] {
  const tabs = [
    ...topic.sessions.map((session): { tab: Tab; at: number } => ({
      tab: { kind: "session", id: session.id, session },
      at: session.opened_at_ms ?? 0,
    })),
    ...(topic.terminals ?? []).map((terminal): { tab: Tab; at: number } => ({
      tab: { kind: "terminal", id: terminal.id, terminal },
      at: terminal.opened_at_ms,
    })),
    ...(topic.diffs ?? []).map((diff): { tab: Tab; at: number } => ({
      tab: { kind: "diff", id: diff.id, diff },
      at: diff.opened_at_ms,
    })),
  ];
  return tabs
    .sort((a, b) => a.at - b.at)
    .flatMap(({ tab }): Tab[] => [
      tab,
      ...(tab.kind === "session" ? (tasks[tab.id] ?? []) : []).map(
        (task): Tab => ({ kind: "task", id: task.id, sessionId: tab.id, task }),
      ),
    ]);
}

export function tabLabel(tab: Tab): string {
  switch (tab.kind) {
    case "session":
      return sessionLabel(tab.session);
    case "terminal":
      return terminalLabel(tab.terminal);
    case "diff":
      return "Diff";
    case "task":
      return taskName(tab.task);
  }
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

/** Conversations to import into a topic. Those from its workspace or its folders are most likely
 * the one, so they come first, each group staying newest first. */
export function conversationsFor(
  conversations: Conversation[],
  workspace: Workspace,
  topic: Topic,
): Conversation[] {
  const folders = [workspace.path, ...topic.worktrees.map((w) => w.path)];
  const inside = (c: Conversation) =>
    folders.some((folder) => c.cwd === folder || c.cwd.startsWith(`${folder}/`));
  return [...conversations.filter(inside), ...conversations.filter((c) => !inside(c))];
}

/** The last part of a path, which is what tells folders apart in a list. */
export function folderName(path: string): string {
  return path.split("/").filter(Boolean).pop() ?? path;
}

export type ActivityStep =
  | { kind: "thought"; key: number; text: string }
  | { kind: "tool"; key: number; tool: ToolCallPatch }
  | { kind: "note"; key: number; text: string }
  /** A subagent the agent started, which has a transcript of its own. */
  | { kind: "subagent"; key: number; id: string };

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
  | { kind: "prompt"; key: number; text: string; images: PromptImage[] }
  | { kind: "message"; key: number; text: string }
  | Activity
  | { kind: "note"; key: number; text: string; failed?: boolean }
  | AnswerEnd
  /** A plan in Markdown, like the one approved on leaving plan mode. */
  | { kind: "plan"; key: number; text: string }
  | {
      kind: "compaction";
      key: number;
      status: string;
      summary?: string;
      error?: string;
    }
  /** A command run with `!`. */
  | { kind: "shell"; key: number; command: string; output: string; exitCode?: number }
  /** Subagents that came back between the agent's messages, with how long each took. */
  | { kind: "subagents_back"; key: number; back: { id: string; tookMs: number }[] };

/** Where an answer finished, which can be branched from when the agent said where. `note` says
 * how it stopped, if that wasn't the usual way. */
export interface AnswerEnd {
  kind: "answer_end";
  key: number;
  note?: string;
  branchable: boolean;
}

const STOP_NOTES: Record<string, string> = {
  cancelled: "Interrupted",
  max_tokens: "Stopped at the token limit",
  max_turn_requests: "Stopped at the turn limit",
  refusal: "The agent refused",
};

/** In a few words, what an answer to a question was, for the transcript. */
export function answerSummary(question: Question | undefined, answer: QuestionAnswer): string {
  switch (answer.action) {
    case "accept": {
      const parts = Object.entries(answer.values).map(([key, value]) => {
        const field = question?.fields?.find((f) => f.key === key);
        const shown = (one: string | number | boolean) => {
          if (field?.kind.type !== "choice") return String(one);
          return field.kind.options.find((o) => o.value === String(one))?.label ?? String(one);
        };
        return Array.isArray(value) ? value.map(shown).join(", ") : shown(value);
      });
      if (answer.note) parts.push(answer.note);
      return parts.length > 0 ? parts.join("; ") : "Answered";
    }
    case "decline":
      return "Skipped";
    case "cancel":
      return "Cancelled";
    default:
      return "Answered";
  }
}

/** Folds a session's events into what a person reads: streamed chunks become one message,
 * and everything an agent does between messages becomes one activity. `view` is a subagent's
 * ID to read its transcript instead of the agent's own. */
export function foldTranscript(events: SessionEvent[], view: string | null = null): TranscriptItem[] {
  const items: TranscriptItem[] = [];
  const tools = new Map<string, Extract<ActivityStep, { kind: "tool" }>>();
  const requests = new Map<string, { title: string; options: Map<string, string> }>();
  const questions = new Map<string, Question>();
  const replaced = new Map<string, TranscriptItem>();
  const subagents = new Map<string, { startedMs: number; status?: SubagentStatus }>();
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

  /** An item that a later event with the same ID updates in place, where it first was. */
  const upsert = (id: string, item: TranscriptItem) => {
    const earlier = replaced.get(id);
    if (earlier) {
      Object.assign(earlier, { ...item, key: earlier.key });
    } else {
      replaced.set(id, item);
      items.push(item);
    }
  };

  const asked = (seq: number, at: number) => {
    activity(seq, at);
    if (requestsOpen++ === 0) waitingSince = at;
  };

  const answered = (seq: number, at: number, text: string) => {
    const current = activity(seq, at);
    if (--requestsOpen === 0) current.waitedMs += at - waitingSince;
    current.steps.push({ kind: "note", key: seq, text });
  };

  for (const { seq, at_ms: at, kind, subagent } of events) {
    if ((subagent ?? null) !== view) continue;
    switch (kind.event) {
      case "user_prompt":
        close(at);
        items.push({ kind: "prompt", key: seq, text: kind.text, images: kind.images ?? [] });
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
      case "subagent": {
        const known = subagents.get(kind.id);
        if (!known) {
          subagents.set(kind.id, { startedMs: at, status: kind.status });
          activity(seq, at).steps.push({ kind: "subagent", key: seq, id: kind.id });
          break;
        }
        touch(at);
        const finished = kind.status === "done" && known.status !== "done";
        if (kind.status !== undefined) known.status = kind.status;
        // Inside an activity its step says so. Between messages, which is where one left
        // running in the background comes back, a line says so, and keeps the message
        // before it from running into the one after.
        if (!finished || open) break;
        const back = { id: kind.id, tookMs: at - known.startedMs };
        const last = items[items.length - 1];
        if (last?.kind === "subagents_back") last.back.push(back);
        else items.push({ kind: "subagents_back", key: seq, back: [back] });
        break;
      }
      case "session_message": {
        // A subagent's task opens its transcript, and its report back closes it. The agent's
        // own side of either shows as the subagent's step.
        if (view === null) break;
        close(at);
        if (kind.to === view) {
          upsert(`message:${kind.id}`, { kind: "prompt", key: seq, text: kind.text, images: [] });
        } else if (kind.from === view) {
          upsert(`message:${kind.id}`, { kind: "message", key: seq, text: kind.text });
        }
        break;
      }
      case "permission_requested":
        requests.set(kind.approval.id, {
          title: kind.approval.tool_call.title ?? "Untitled tool call",
          options: new Map(kind.approval.options.map((option) => [option.id, option.name])),
        });
        asked(seq, at);
        break;
      case "permission_resolved": {
        const request = requests.get(kind.approval_id);
        const answer =
          kind.outcome.outcome === "selected"
            ? (request?.options.get(kind.outcome.option_id) ?? "Answered")
            : "Cancelled";
        answered(seq, at, `Request: ${request?.title ?? "tool call"}. ${answer}`);
        break;
      }
      case "question_asked":
        questions.set(kind.question.id, kind.question);
        asked(seq, at);
        break;
      case "question_answered": {
        const question = questions.get(kind.question_id);
        answered(
          seq,
          at,
          `Question: ${question?.message ?? "a question"} ${answerSummary(question, kind.answer)}`,
        );
        break;
      }
      case "plan_document":
        close(at);
        upsert(`plan:${kind.id}`, { kind: "plan", key: seq, text: kind.text });
        break;
      case "compaction":
        close(at);
        upsert(`compaction:${kind.id}`, {
          kind: "compaction",
          key: seq,
          status: kind.status,
          summary: kind.summary,
          error: kind.error,
        });
        break;
      case "notice":
        close(at);
        items.push({
          kind: "note",
          key: seq,
          text: kind.description ? `${kind.title}. ${kind.description}` : kind.title,
          failed: kind.severity === "error",
        });
        break;
      case "shell":
        close(at);
        items.push({
          kind: "shell",
          key: seq,
          command: kind.command,
          output: kind.output,
          exitCode: kind.exit_code,
        });
        break;
      case "agent_ready":
        close(at);
        break;
      case "turn_ended": {
        close(at);
        items.push({
          kind: "answer_end",
          key: seq,
          note: STOP_NOTES[kind.stop_reason],
          branchable: kind.last_message !== undefined,
        });
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

export interface SubagentInfo {
  id: string;
  title?: string;
  description?: string;
  status?: SubagentStatus;
  /** When it was first heard of, which is when it started. */
  startedMs: number;
}

/** Every subagent a session's agent started, as their latest reports describe them, in the
 * order they started. */
export function subagentsOf(events: SessionEvent[]): Map<string, SubagentInfo> {
  const subagents = new Map<string, SubagentInfo>();
  for (const { kind, at_ms } of events) {
    if (kind.event !== "subagent") continue;
    const known = subagents.get(kind.id) ?? { id: kind.id, startedMs: at_ms };
    subagents.set(kind.id, {
      ...known,
      ...(kind.title !== undefined && { title: kind.title }),
      ...(kind.description !== undefined && { description: kind.description }),
      ...(kind.status !== undefined && { status: kind.status }),
    });
  }
  return subagents;
}

export const subagentLabel = (subagent: SubagentInfo | undefined) =>
  subagent?.title ?? subagent?.description ?? "Subagent";

/** Whether a subagent is still going, so its step stays in sight. */
export const subagentRunning = (subagent: SubagentInfo | undefined) =>
  subagent?.status === "working" || subagent?.status === "waiting_for_you";

/** How a subagent's status reads as one of the app's states. */
export function subagentState(status: SubagentStatus | undefined, online: boolean): StatusState {
  if (!online) return "stale";
  switch (status) {
    case "working":
      return "working";
    case "waiting_for_you":
      return "waiting";
    case "done":
      return "read";
    default:
      return "idle";
  }
}

/** The agent's own todo list as it last reported it, or null if it never did. */
export function planOf(events: SessionEvent[]): PlanEntry[] | null {
  for (let i = events.length - 1; i >= 0; i--) {
    const { kind, subagent } = events[i];
    if (kind.event === "plan" && subagent === undefined) return kind.entries;
  }
  return null;
}

/** How full the agent's context is, as a whole percentage, with what it's made of. */
export function usageOf(
  events: SessionEvent[],
): { percent: number; used: number; size: number; cost?: string } | null {
  for (let i = events.length - 1; i >= 0; i--) {
    const { kind } = events[i];
    if (kind.event !== "usage" || kind.size <= 0) continue;
    const cost = kind.cost ? `${kind.cost.amount.toFixed(2)} ${kind.cost.currency}` : undefined;
    return {
      percent: Math.min(100, Math.round((kind.used / kind.size) * 100)),
      used: kind.used,
      size: kind.size,
      cost,
    };
  }
  return null;
}

/** What the agent expects to be sent next, until something is. */
export function suggestionOf(events: SessionEvent[]): string | null {
  for (let i = events.length - 1; i >= 0; i--) {
    const { kind } = events[i];
    if (kind.event === "suggestion") return kind.text?.trim() || null;
    if (kind.event === "user_prompt" || kind.event === "shell") return null;
  }
  return null;
}

const plural = (count: number, one: string, many = `${one}s`) =>
  `${count} ${count === 1 ? one : many}`;

/** The line the todo list folds to, or null when it's all done and the list goes away,
 * since the transcript has what was done. */
export function todoSummary(plan: PlanEntry[] | null): string | null {
  const todo = plan ?? [];
  const done = todo.filter((entry) => entry.status === "completed").length;
  return done < todo.length ? `${done} of ${plural(todo.length, "todo")} done` : null;
}

/** How a background task's status reads as one of the app's states. */
export function taskState(status: TaskStatus, online: boolean): StatusState {
  if (!online) return "stale";
  switch (status) {
    case "running":
      return "working";
    case "failed":
      return "failed";
    case "completed":
      return "read";
    default:
      return "idle";
  }
}

export const taskName = (task: BackgroundTask) => task.description || task.command || task.kind;

/** A large count in the app's short form, like 12k. */
export function shortCount(count: number): string {
  if (count < 1000) return String(count);
  if (count < 1_000_000) return `${Math.round(count / 1000)}k`;
  return `${(count / 1_000_000).toFixed(1)}M`;
}

/** The agent's own options a session can set, which only a running agent reports. */
export function sessionOptions(state: DaemonState | null, sessionId: string): AgentOption[] {
  return state?.session_options?.[sessionId] ?? [];
}

/** Why an answer can't be branched from, or `undefined` when it can. */
export function branchBlocked(
  item: AnswerEnd,
  provider: Provider | undefined,
  agent: string,
  online: boolean,
): string | undefined {
  if (!online) return "The device is offline";
  if (!provider?.can_fork) return `${provider?.name ?? agent} can't branch sessions`;
  if (!item.branchable) return "The agent didn't say where this answer ends, so it can't be branched";
  return undefined;
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
  if (session.status === "starting") return "Starting";
  const steps = (activity?.steps ?? []).filter((step) => step.kind !== "note");
  const running = steps.filter(
    (step) =>
      step.kind !== "tool" || (step.tool.status !== "completed" && step.tool.status !== "failed"),
  );
  const step = running[running.length - 1] ?? steps[steps.length - 1];
  if (!step) return "Working";
  if (step.kind === "subagent") return "Working with a subagent";
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
