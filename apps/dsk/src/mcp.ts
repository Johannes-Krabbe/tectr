// What the MCP screens show and send, worked out from the daemon's state. No DOM or Tauri in
// here, so it can be tested on its own.

import type { Command, DaemonState, McpServer, McpTransport, Topic } from "./engine";

/** The servers every topic on the device can get. */
export function deviceServers(state: DaemonState): McpServer[] {
  return (state.mcp_servers ?? []).filter((server) => !server.topic_id);
}

/** The servers a topic has to itself. */
export function ownServers(state: DaemonState, topicId: string): McpServer[] {
  return (state.mcp_servers ?? []).filter((server) => server.topic_id === topicId);
}

/** How a topic treats one of the device's servers: following the server's own default, or
 * turned on or off for itself. */
export type TopicMcpChoice = "default" | "on" | "off";

export function topicMcpChoice(topic: Topic, server: McpServer): TopicMcpChoice {
  const set = topic.mcp?.[server.id];
  return set === undefined ? "default" : set ? "on" : "off";
}

/** Whether the topic's agents get one of the device's servers. */
export function topicGets(topic: Topic, server: McpServer): boolean {
  return topic.mcp?.[server.id] ?? server.enabled;
}

/** The servers a topic's agents get, the same as the daemon's `topic_mcp_servers`: the
 * device's that are on for it, then its own that are on. */
export function topicMcpServers(state: DaemonState, topic: Topic): McpServer[] {
  return (state.mcp_servers ?? []).filter((server) =>
    server.topic_id ? server.topic_id === topic.id && server.enabled : topicGets(topic, server),
  );
}

export function topicMcpLabel(topic: Topic, server: McpServer): string {
  switch (topicMcpChoice(topic, server)) {
    case "default":
      return server.enabled ? "On by default" : "Off by default";
    case "on":
      return "On here";
    case "off":
      return "Off here";
  }
}

/** What `set_topic_mcp` sends to flip whether the topic gets a device server. Ending up where
 * the default is means following it again, so a flip can always be undone by flipping. */
export function flippedTopicMcp(topic: Topic, server: McpServer): boolean | null {
  const next = !topicGets(topic, server);
  return next === server.enabled ? null : next;
}

/** Quotes an argument only when it has to be, so it reads back as the same one. */
function quoteArg(arg: string): string {
  if (arg !== "" && !/[\s"'\\]/.test(arg)) return arg;
  return `"${arg.replace(/["\\]/g, (char) => `\\${char}`)}"`;
}

export function joinArgs(args: string[]): string {
  return args.map(quoteArg).join(" ");
}

/** Splits arguments the way a shell would, with quotes and backslashes, and nothing else
 * expanded, because the daemon runs them without a shell. */
export function splitArgs(text: string): { args: string[] } | { error: string } {
  const args: string[] = [];
  let current = "";
  let started = false;
  let quote: '"' | "'" | null = null;
  for (let index = 0; index < text.length; index++) {
    const char = text[index];
    if (quote === "'") {
      if (char === "'") quote = null;
      else current += char;
    } else if (char === "\\" && index + 1 < text.length && (quote === null || /["\\]/.test(text[index + 1]))) {
      current += text[++index];
      started = true;
    } else if (quote === '"') {
      if (char === '"') quote = null;
      else current += char;
    } else if (char === '"' || char === "'") {
      quote = char;
      started = true;
    } else if (/\s/.test(char)) {
      if (started) args.push(current);
      current = "";
      started = false;
    } else {
      current += char;
      started = true;
    }
  }
  if (quote) return { error: `An argument has a ${quote} without its closing one` };
  if (started) args.push(current);
  return { args };
}

/** One `KEY=VALUE` a line. Blank lines are skipped, and what's around the key and the value
 * is trimmed, so `KEY = value` reads the same. */
export function parsePairs(text: string, what: string): { pairs: Record<string, string> } | { error: string } {
  const pairs: Record<string, string> = {};
  const lines = text.split("\n");
  for (const [index, line] of lines.entries()) {
    if (line.trim() === "") continue;
    const at = line.indexOf("=");
    const key = at === -1 ? "" : line.slice(0, at).trim();
    if (key === "") return { error: `${what}: line ${index + 1} needs KEY=VALUE` };
    if (key in pairs) return { error: `${what}: ${key} is there twice` };
    pairs[key] = line.slice(at + 1).trim();
  }
  return { pairs };
}

export function formatPairs(pairs: Record<string, string> | undefined): string {
  return Object.entries(pairs ?? {})
    .map(([key, value]) => `${key}=${value}`)
    .join("\n");
}

/** What a server runs or where it is, in one line. */
export function mcpSummary(transport: McpTransport): string {
  switch (transport.type) {
    case "stdio":
      return [quoteArg(transport.command), ...(transport.args ?? []).map(quoteArg)].join(" ");
    case "http":
      return transport.url;
    case "unknown":
      return "A kind of server this app doesn't know";
  }
}

export function mcpAuthLabel(server: McpServer): string | null {
  if (server.transport.type !== "http") return null;
  switch (server.auth) {
    case "needs_login":
      return "Needs login";
    case "logged_in":
      return "Logged in";
    default:
      return null;
  }
}

/** Saves a server as it is but on or off. Null for a kind of server this app can't send back. */
export function withEnabled(server: McpServer, enabled: boolean): Command | null {
  if (server.transport.type === "unknown") return null;
  return {
    command: "save_mcp_server",
    id: server.id,
    name: server.name,
    ...(server.topic_id && { topic_id: server.topic_id }),
    transport: server.transport,
    enabled,
  };
}

export const canEdit = (server: McpServer) => server.transport.type !== "unknown";

export const canLogIn = (server: McpServer) => server.transport.type === "http";

export const canLogOut = (server: McpServer) =>
  server.transport.type === "http" && server.auth === "logged_in";

/** A server as typed into the form. Both kinds keep their fields, so switching back and forth
 * loses nothing. */
export interface McpDraft {
  name: string;
  type: "stdio" | "http";
  command: string;
  args: string;
  env: string;
  url: string;
  headers: string;
  oauthClientId: string;
  enabled: boolean;
}

export function draftOf(server: McpServer | undefined): McpDraft {
  const draft: McpDraft = {
    name: server?.name ?? "",
    type: "stdio",
    command: "",
    args: "",
    env: "",
    url: "",
    headers: "",
    oauthClientId: "",
    enabled: server?.enabled ?? true,
  };
  const transport = server?.transport;
  if (transport?.type === "stdio") {
    return {
      ...draft,
      command: transport.command,
      args: joinArgs(transport.args ?? []),
      env: formatPairs(transport.env),
    };
  }
  if (transport?.type === "http") {
    return {
      ...draft,
      type: "http",
      url: transport.url,
      headers: formatPairs(transport.headers),
      oauthClientId: transport.oauth_client_id ?? "",
    };
  }
  return draft;
}

const NAME = /^[A-Za-z0-9_-]+$/;

/** What's wrong with a name, or null when it's fine or not typed yet. */
export function nameProblem(name: string): string | null {
  return name === "" || NAME.test(name) ? null : "Names use letters, digits, - and _";
}

/** Whether the form has what saving needs. The daemon checks the rest and says what's wrong. */
export function draftReady(draft: McpDraft): boolean {
  const target = draft.type === "stdio" ? draft.command : draft.url;
  return NAME.test(draft.name.trim()) && target.trim() !== "";
}

/** The transport a draft describes, leaving out what's empty. */
export function transportOf(draft: McpDraft): { transport: McpTransport } | { error: string } {
  if (draft.type === "stdio") {
    const args = splitArgs(draft.args);
    if ("error" in args) return args;
    const env = parsePairs(draft.env, "Environment");
    if ("error" in env) return env;
    return {
      transport: {
        type: "stdio",
        command: draft.command.trim(),
        ...(args.args.length > 0 && { args: args.args }),
        ...(Object.keys(env.pairs).length > 0 && { env: env.pairs }),
      },
    };
  }
  const headers = parsePairs(draft.headers, "Headers");
  if ("error" in headers) return headers;
  const clientId = draft.oauthClientId.trim();
  return {
    transport: {
      type: "http",
      url: draft.url.trim(),
      ...(Object.keys(headers.pairs).length > 0 && { headers: headers.pairs }),
      ...(clientId !== "" && { oauth_client_id: clientId }),
    },
  };
}

/** What the import said, as lines to show. */
export function importLines(added: string[], skipped: string[]): string[] {
  const lines = [added.length > 0 ? `Added ${added.join(", ")}` : "Nothing new to add"];
  return [...lines, ...skipped.map((reason) => `Skipped ${reason}`)];
}
