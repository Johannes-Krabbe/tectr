// The webview's view of the engine. These types mirror crates/protocol and are
// written by hand for now. Unknown values from newer daemons arrive as "unknown".

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";

export type PermissionMode = "manual" | "auto" | "yolo" | "unknown";

export type SessionStatus =
  | "starting"
  | "ready"
  | "working"
  | "waiting_for_you"
  | "done"
  | "failed"
  | "stopped"
  | "unknown";

/** One agent run, in its topic's folder. */
export interface Session {
  id: string;
  agent: string;
  mode: PermissionMode;
  model: string | null;
  effort?: string | null;
  status: SessionStatus;
  title: string | null;
  /** A turn finished and nobody has looked at it yet, on any client. */
  unread?: boolean;
  /** Puts its tab in order among the topic's agents and terminals. */
  opened_at_ms?: number;
}

export type TerminalStatus = "running" | "stopped" | "exited" | "unknown";

/** A shell in its topic's folder. */
export interface Terminal {
  id: string;
  /** What the program in it last called it. */
  title?: string | null;
  /** Stopped: no shell since the daemon started, and showing it starts one. Exited: its shell
   * failed, and typing starts another. */
  status: TerminalStatus;
  opened_at_ms: number;
}

/** A client's colors as `#rrggbb`, which programs ask a terminal for. */
export interface TerminalColors {
  foreground: string;
  background: string;
  cursor: string;
  /** The 16 ANSI colors, black to bright white. */
  palette: string[];
}

/** A slash command an agent offers. Sending `/name`, and anything after it, runs it. */
export interface AgentCommand {
  name: string;
  description: string;
  /** What to type after it, for a command that takes input. */
  hint?: string | null;
}

export interface Worktree {
  id: string;
  path: string;
  /** What it has checked out now, which an agent can change. `HEAD` when that's no branch. */
  branch: string;
}

/** All its tabs work in one folder at a time. */
export interface Topic {
  id: string;
  name: string;
  /** The folder its tabs work in, one of `worktrees`. */
  worktree_id: string;
  /** Every folder it has worked in and still has. One at the repo's own path is the repo. */
  worktrees: Worktree[];
  sessions: Session[];
  /** Left out while it has none. */
  terminals?: Terminal[];
  /** The device's MCP servers this topic turns on or off, against their own `enabled`. By
   * server ID. */
  mcp?: Record<string, boolean>;
}

export type MoveTarget =
  | { kind: "new_worktree" }
  | { kind: "root" }
  | { kind: "worktree"; worktree_id: string };

/** What archiving a topic would lose. */
export interface ArchiveRisk {
  /** Agents starting, working or waiting, which archiving stops. */
  running: number;
  /** Only worktrees with something to lose. */
  worktrees: WorktreeRisk[];
}

export interface WorktreeRisk {
  path: string;
  /** The branch tectr made for the worktree, which archiving deletes. */
  branch: string;
  uncommitted: number;
  /** Commits that no other branch or remote has. */
  unpushed: number;
}

export interface ArchivedTopic {
  id: string;
  workspace_id: string;
  name: string;
  archived_at_ms: number;
}

/** A conversation Claude Code or Codex keeps itself, which can be imported into a topic. */
export interface Conversation {
  /** The agent that carries it on. */
  agent: string;
  /** The agent's own ID for it. */
  id: string;
  title: string;
  /** The folder it was in. */
  cwd: string;
  updated_at_ms: number;
}

export interface Workspace {
  id: string;
  name: string;
  path: string;
  topics: Topic[];
}

export interface ToolCallPatch {
  title?: string;
  kind?: string;
  status?: "pending" | "in_progress" | "completed" | "failed" | "unknown";
  locations?: string[];
}

export interface PermissionOption {
  id: string;
  name: string;
  kind: "allow_once" | "allow_always" | "reject_once" | "reject_always" | "unknown";
}

export interface Approval {
  id: string;
  session_id: string;
  tool_call: ToolCallPatch;
  options: PermissionOption[];
}

export interface ProviderEffort {
  id: string;
  name: string;
}

export interface ProviderModel {
  id: string;
  name: string;
  efforts: ProviderEffort[];
}

/** An agent added to a device, with what it reported when last started. */
export interface Provider {
  agent: string;
  name: string;
  version: string | null;
  modes: PermissionMode[];
  models: ProviderModel[];
  default_model: string | null;
  default_effort?: string | null;
  recommended_model?: string | null;
}

export interface ProviderCandidate {
  agent: string;
  name: string;
  missing: string | null;
}

/** A folder on a device. Not being a repo can also mean the daemon didn't look. */
export interface Folder {
  name: string;
  path: string;
  is_repo: boolean;
}

export interface DaemonState {
  machine: { id: string; name: string };
  workspaces: Workspace[];
  approvals: Approval[];
  providers?: Provider[];
  /** What's typed for each session but not sent yet. */
  drafts?: Record<string, string>;
  /** The slash commands each session's agent last said it has. */
  commands?: Record<string, AgentCommand[]>;
  /** Left out while every value is its default. */
  settings?: DeviceSettings;
  /** The MCP servers agents on this device get, device-wide and per topic. */
  mcp_servers?: McpServer[];
  /** The clients on other machines that may connect, oldest first. */
  paired_clients?: PairedClient[];
}

/** A client on another machine that may connect to a daemon, by the key it proves. */
export interface PairedClient {
  key: string;
  name: string;
  paired_at_ms: number;
}

/** How agents reach an MCP server. Values may name the daemon's environment as `${NAME}`. */
export type McpTransport =
  | { type: "stdio"; command: string; args?: string[]; env?: Record<string, string> }
  | {
      type: "http";
      url: string;
      headers?: Record<string, string>;
      /** For a server that doesn't register clients by itself. */
      oauth_client_id?: string | null;
    }
  | { type: "unknown" };

/** Whether the daemon holds a login. Left out while it's `none`. */
export type McpAuth = "none" | "needs_login" | "logged_in" | "unknown";

/** An MCP server that tectr gives agents, the only ones they get. */
export interface McpServer {
  id: string;
  name: string;
  /** The topic that has it to itself. Left out for a device-wide server. */
  topic_id?: string | null;
  transport: McpTransport;
  /** Device-wide: whether topics get it unless they turn it off. A topic's own: whether it's on. */
  enabled: boolean;
  auth?: McpAuth;
}

/** A device's own settings. A field left out keeps its default. */
export interface DeviceSettings {
  /** Defaults to on. */
  keep_awake?: boolean;
}

export type Connection =
  | { state: "connecting" }
  | { state: "online" }
  | { state: "offline"; reason: string }
  | { state: "incompatible"; daemon_min: number; daemon_max: number };

export interface Device {
  url: string;
  connection: Connection;
  state: DaemonState | null;
  last_seen_ms: number | null;
}

export interface SessionRef {
  machine_id: string;
  session_id: string;
}

export type SessionEventKind =
  | { event: "status_changed"; status: SessionStatus }
  | { event: "agent_ready"; name: string; version: string | null; current_mode: string | null }
  | { event: "user_prompt"; text: string }
  | { event: "agent_message"; text: string }
  | { event: "agent_thought"; text: string }
  | { event: "tool_call"; id: string; tool: ToolCallPatch }
  | { event: "tool_call_update"; id: string; tool: ToolCallPatch }
  | { event: "permission_requested"; approval: Approval }
  | {
      event: "permission_resolved";
      approval_id: string;
      outcome: { outcome: "selected"; option_id: string } | { outcome: "cancelled" | "unknown" };
    }
  | { event: "turn_ended"; stop_reason: string }
  | { event: "error"; message: string }
  | { event: "unknown" };

export type SessionEvent = { seq: number; at_ms: number; kind: SessionEventKind };

export type Command =
  | { command: "add_workspace"; path: string }
  | { command: "list_folders"; path: string }
  | { command: "create_topic"; workspace_id: string; name: string | null; at_root: boolean }
  | { command: "rename_topic"; topic_id: string; name: string }
  | { command: "archive_topic"; topic_id: string; force?: boolean }
  | { command: "restore_topic"; topic_id: string }
  | { command: "list_archived_topics" }
  | { command: "move_topic"; topic_id: string; to: MoveTarget; force?: boolean }
  | {
      command: "start_session";
      topic_id: string;
      agent: string;
      mode: PermissionMode;
      model: string | null;
      effort?: string | null;
    }
  | { command: "send_prompt"; session_id: string; text: string }
  | { command: "cancel_turn"; session_id: string }
  | { command: "answer_approval"; approval_id: string; option_id: string }
  | { command: "rename_machine"; name: string }
  | { command: "set_device_settings"; settings: DeviceSettings }
  | { command: "detect_providers" }
  | { command: "add_provider"; agent: string }
  | { command: "remove_provider"; agent: string }
  | {
      command: "set_provider_defaults";
      agent: string;
      model: string | null;
      effort: string | null;
    }
  | {
      command: "set_session_settings";
      session_id: string;
      mode: PermissionMode;
      model: string | null;
      effort: string | null;
    }
  | { command: "set_unread"; session_id: string; unread: boolean }
  | { command: "set_draft"; session_id: string; text: string }
  | { command: "close_session"; session_id: string; force?: boolean }
  | { command: "open_terminal"; topic_id: string; cols: number; rows: number }
  | { command: "close_terminal"; terminal_id: string }
  | {
      command: "save_mcp_server";
      /** Changes the server it names, or else adds one. */
      id?: string;
      name: string;
      /** Left out for a device-wide server. A server can't move between the two. */
      topic_id?: string;
      transport: McpTransport;
      enabled: boolean;
    }
  | { command: "remove_mcp_server"; id: string }
  /** Null follows the server's own `enabled` again. */
  | { command: "set_topic_mcp"; topic_id: string; server_id: string; on: boolean | null }
  | { command: "login_mcp_server"; id: string }
  | { command: "complete_mcp_login"; state: string; code: string }
  | { command: "logout_mcp_server"; id: string }
  | { command: "import_mcp_servers" }
  | { command: "list_conversations" }
  | { command: "import_conversation"; topic_id: string; agent: string; id: string }
  | { command: "start_pairing" }
  | { command: "cancel_pairing" }
  | { command: "remove_paired_client"; key: string };

export type CommandOutput =
  | { output: "workspace"; workspace: Workspace }
  | { output: "topic"; workspace_id: string; topic: Topic }
  | { output: "session"; topic_id: string; session: Session }
  | { output: "terminal"; topic_id: string; terminal: Terminal }
  | { output: "provider_candidates"; candidates: ProviderCandidate[] }
  | { output: "provider"; provider: Provider }
  | { output: "folders"; folders: Folder[] }
  | { output: "archive_risk"; risk: ArchiveRisk }
  | { output: "archived_topics"; topics: ArchivedTopic[] }
  | { output: "mcp_server"; server: McpServer }
  /** Where the browser has to go to log in. */
  | { output: "mcp_login"; url: string }
  /** The names added, and why each one left out was. */
  | { output: "mcp_import"; added: string[]; skipped: string[] }
  /** Newest first. */
  | { output: "conversations"; conversations: Conversation[] }
  /** A code that pairs one more machine, once, until it expires. */
  | { output: "pairing"; code: string; expires_at_ms: number }
  | { output: "done" }
  | { output: "unknown" };

/** Rejects with the daemon's message, which is written to be shown as is. */
export function request(machineId: string, command: Command): Promise<CommandOutput> {
  return invoke("request", { machineId, command });
}

/** Pairs with a device on another machine using the code it showed, and keeps it. */
export function pairDevice(code: string): Promise<void> {
  return invoke("pair_device", { code });
}

/** Stops reaching a paired device, taking this app off its allowed list if it's online. */
export function forgetDevice(url: string): Promise<void> {
  return invoke("forget_device", { url });
}

/** The key this app proves to other machines, which their paired clients list. */
export function clientKey(): Promise<string> {
  return invoke("client_key");
}

export function errorText(error: unknown): string {
  return typeof error === "string" ? error : String(error);
}

/** The device tree, kept current. It listens before reading, so no change is missed. */
export function useDevices(): Device[] {
  const [devices, setDevices] = useState<Device[]>([]);
  useEffect(() => {
    let latest = 0;
    let active = true;
    const read = async () => {
      const id = ++latest;
      const next = await invoke<Device[]>("devices");
      if (active && id === latest) setDevices(next);
    };
    const stop = listen<boolean>("engine-changed", (event) => {
      if (event.payload) void read();
    });
    void stop.then(read);
    return () => {
      active = false;
      void stop.then((unlisten) => unlisten());
    };
  }, []);
  return devices;
}

/** One session's history, kept current. Remount for another session. */
export function useSessionEvents(session: SessionRef): SessionEvent[] {
  const { machine_id, session_id } = session;
  const [events, setEvents] = useState<SessionEvent[]>([]);
  useEffect(() => {
    const session = { machine_id, session_id };
    let active = true;
    let last = 0;
    const read = async () => {
      const fresh = await invoke<SessionEvent[]>("events_after", { session, afterSeq: last });
      // Two reads can overlap, so keep only what's still new.
      const next = fresh.filter((event) => event.seq > last);
      if (!active || next.length === 0) return;
      last = next[next.length - 1].seq;
      setEvents((previous) => [...previous, ...next]);
    };
    const stop = listen("engine-changed", () => void read());
    void stop.then(async () => {
      await invoke("open_session", { session });
      await read();
    });
    return () => {
      active = false;
      void stop.then((unlisten) => unlisten());
    };
  }, [machine_id, session_id]);
  return events;
}
