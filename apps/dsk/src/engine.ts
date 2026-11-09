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

export interface Session {
  id: string;
  worktree_id: string;
  agent: string;
  mode: PermissionMode;
  model: string | null;
  effort?: string | null;
  status: SessionStatus;
  title: string | null;
}

export interface Worktree {
  id: string;
  path: string;
  branch: string;
}

export interface Topic {
  id: string;
  name: string;
  worktrees: Worktree[];
  sessions: Session[];
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
  | { command: "delete_topic"; topic_id: string; force?: boolean }
  | {
      command: "start_session";
      topic_id: string;
      worktree_id: string;
      agent: string;
      mode: PermissionMode;
      model: string | null;
      effort?: string | null;
    }
  | { command: "send_prompt"; session_id: string; text: string }
  | { command: "cancel_turn"; session_id: string }
  | { command: "answer_approval"; approval_id: string; option_id: string }
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
  | { command: "set_draft"; session_id: string; text: string }
  | { command: "close_session"; session_id: string; force?: boolean };

export type CommandOutput =
  | { output: "workspace"; workspace: Workspace }
  | { output: "topic"; workspace_id: string; topic: Topic }
  | { output: "session"; topic_id: string; session: Session }
  | { output: "provider_candidates"; candidates: ProviderCandidate[] }
  | { output: "provider"; provider: Provider }
  | { output: "folders"; folders: Folder[] }
  | { output: "done" }
  | { output: "unknown" };

/** Rejects with the daemon's message, which is written to be shown as is. */
export function request(machineId: string, command: Command): Promise<CommandOutput> {
  return invoke("request", { machineId, command });
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
