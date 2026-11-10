// What the MCP screens show and send, worked out from the daemon's state. No DOM or Tauri in
// here, so it can be tested on its own.

import type { Command, DaemonState, McpServer, McpTransport, Topic } from "./engine";

export const isConnector = (server: McpServer) =>
  server.transport.type === "claude_connector" || server.transport.type === "codex_app";

/** The servers every topic on the device can get, apart from connectors. */
export function deviceServers(state: DaemonState): McpServer[] {
  return (state.mcp_servers ?? []).filter((server) => !server.topic_id && !isConnector(server));
}

/** The claude.ai connectors and ChatGPT apps, which only their own agent gets. */
export function connectors(state: DaemonState): McpServer[] {
  return (state.mcp_servers ?? []).filter(isConnector);
}

/** The agent a connector is for. */
export function connectorAgent(server: McpServer): string | null {
  switch (server.transport.type) {
    case "claude_connector":
      return "Claude";
    case "codex_app":
      return "Codex";
    default:
      return null;
  }
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
    case "claude_connector":
      return transport.url;
    case "codex_app":
      return "ChatGPT app";
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

export const canToggle = (server: McpServer) => server.transport.type !== "unknown";

/** A connector is its agent's, so it's only turned on or off. */
export const canEdit = (server: McpServer) => canToggle(server) && !isConnector(server);

/** A removed connector would come back with the next refresh. */
export const canRemove = (server: McpServer) => !isConnector(server);

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

/** What refreshing the connectors said, as lines to show. */
export function connectorLines(added: string[], removed: string[], problems: string[]): string[] {
  const lines: string[] = [];
  if (added.length > 0) lines.push(`Added ${added.join(", ")}, off until turned on`);
  if (removed.length > 0) lines.push(`Removed ${removed.join(", ")}`);
  if (lines.length === 0 && problems.length === 0) lines.push("No new connectors");
  return [...lines, ...problems.map((problem) => `Couldn't list: ${problem}`)];
}

// Reading a server from whatever a README hands you: an address, a command, a `claude mcp add`
// or `codex mcp add` line, or a JSON block in the format most docs show.

/** A server found in what was pasted, not added yet. */
export interface Found {
  name: string;
  transport: McpTransport;
  /** Env variables and headers whose value is still a placeholder to fill in. */
  blanks: Blank[];
}

export interface Blank {
  in: "env" | "headers";
  key: string;
}

export type Pasted = { found: Found[] } | { error: string } | { empty: true };

/** What a value still to be filled in looks like in docs: `<token>`, `YOUR_KEY`, `xxx`, or
 * nothing at all. A `${NAME}` is a real value: the daemon fills it in from its environment. */
export function isBlank(value: string): boolean {
  return value.trim() === "" || /<[^>]+>|\byour[_-]|\bxxx+\b|\.\.\./i.test(value);
}

/** Letters, digits, `-` and `_`, the way agents need a server's name. */
export function cleanName(text: string): string {
  return text
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, "-")
    .replace(/^[-_]+|[-_]+$/g, "")
    .slice(0, 64);
}

const HOST_NOISE = new Set(["mcp", "api", "www", "server", "app", "remote"]);

/** `https://mcp.linear.app/mcp` is linear: the host without its top level and its generic
 * labels. */
export function nameFromUrl(url: string): string {
  let host: string;
  try {
    host = new URL(url).hostname;
  } catch {
    return "server";
  }
  const labels = host.split(".");
  const named = labels.length > 1 ? labels.slice(0, -1) : labels;
  const meaningful = named.filter((label) => !HOST_NOISE.has(label));
  return cleanName(meaningful.at(-1) ?? named[0] ?? host) || "server";
}

const RUNNERS = new Set(["npx", "bunx", "uvx", "pipx", "dlx", "deno", "node", "python", "python3"]);

/** `npx -y @modelcontextprotocol/server-github` is github: the package without its scope,
 * version and the words every MCP package has. */
export function nameFromCommand(command: string, args: string[]): string {
  const base = command.split("/").at(-1) ?? command;
  const words = RUNNERS.has(base) || base === "pnpm" || base === "docker" ? args : [base];
  const target = words.find((word) => !word.startsWith("-") && !["dlx", "run", "x"].includes(word));
  if (!target) return cleanName(base) || "server";
  const withoutVersion = target.replace(/(.)@[^/]*$/, "$1");
  const [scope, name = scope] = withoutVersion.replace(/^@/, "").split("/");
  const stripped = name
    .replace(/^(mcp-server-|server-|mcp-)/, "")
    .replace(/(-mcp-server|-server|-mcp|\.py|\.js)$/, "");
  // `@playwright/mcp` says nothing in its name, so its scope is the name.
  const pick = stripped === "" || stripped === "mcp" ? scope : stripped;
  return cleanName(pick) || "server";
}

function blanksOf(transport: McpTransport): Blank[] {
  const blanks: Blank[] = [];
  if (transport.type === "stdio") {
    for (const [key, value] of Object.entries(transport.env ?? {})) {
      if (isBlank(value)) blanks.push({ in: "env", key });
    }
  }
  if (transport.type === "http") {
    for (const [key, value] of Object.entries(transport.headers ?? {})) {
      if (isBlank(value)) blanks.push({ in: "headers", key });
    }
  }
  return blanks;
}

function found(name: string, transport: McpTransport): Found {
  return { name, transport, blanks: blanksOf(transport) };
}

const isUrl = (text: string) => /^https?:\/\/\S+$/i.test(text);

function stringRecord(value: unknown): Record<string, string> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return {};
  const out: Record<string, string> = {};
  for (const [key, inner] of Object.entries(value)) {
    if (typeof inner === "string") out[key] = inner;
    else if (typeof inner === "number" || typeof inner === "boolean") out[key] = String(inner);
  }
  return out;
}

const strings = (value: unknown): string[] =>
  Array.isArray(value) ? value.filter((item): item is string => typeof item === "string") : [];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** One server's entry, in the shapes Claude, Cursor, VS Code, Codex and OpenCode use. */
function fromEntry(name: string | undefined, entry: Record<string, unknown>): Found | string {
  const type = typeof entry.type === "string" ? entry.type : undefined;
  const url = [entry.url, entry.serverUrl, entry.httpUrl].find((u) => typeof u === "string");
  if (type === "sse") return `${name ?? "This server"} uses SSE, which tectr doesn't support`;
  if (typeof url === "string") {
    return found(name ?? nameFromUrl(url), {
      type: "http",
      url,
      headers: stringRecord(entry.headers),
    });
  }
  // OpenCode keeps the program and its arguments in one list.
  const command = Array.isArray(entry.command) ? strings(entry.command) : [];
  const program = typeof entry.command === "string" ? entry.command : command[0];
  if (!program) return `${name ?? "This server"} has neither a command nor a url`;
  const args = typeof entry.command === "string" ? strings(entry.args) : command.slice(1);
  const env = { ...stringRecord(entry.env), ...stringRecord(entry.environment) };
  return found(name ?? nameFromCommand(program, args), { type: "stdio", command: program, args, env });
}

const SERVER_LISTS = ["mcpServers", "servers", "mcp", "context_servers", "mcp_servers"];

function fromJson(value: unknown): Pasted {
  if (!isRecord(value)) return { error: "That JSON isn't an MCP server or a list of them" };
  const list = SERVER_LISTS.map((key) => value[key]).find(isRecord);
  const looksLikeEntry = "command" in value || "url" in value || "serverUrl" in value;
  const entries: [string | undefined, unknown][] = list
    ? Object.entries(list)
    : looksLikeEntry
      ? [[undefined, value]]
      : Object.entries(value);
  const servers: Found[] = [];
  const problems: string[] = [];
  for (const [name, entry] of entries) {
    if (!isRecord(entry)) continue;
    const result = fromEntry(name === undefined ? undefined : cleanName(name) || undefined, entry);
    if (typeof result === "string") problems.push(result);
    else servers.push(result);
  }
  if (servers.length > 0) return { found: servers };
  return { error: problems[0] ?? "That JSON has no MCP servers in it" };
}

/** Docs paste JSON with comments and trailing commas, which JSON.parse refuses. */
function parseLooseJson(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    const loose = text
      .replace(/("(?:\\.|[^"\\])*")|\/\/[^\n]*|\/\*[\s\S]*?\*\//g, (_, string) => string ?? "")
      .replace(/("(?:\\.|[^"\\])*")|,(\s*[}\]])/g, (_, string, close) => string ?? close);
    return JSON.parse(loose);
  }
}

function pairOf(text: string, separator: "=" | ":"): [string, string] | null {
  const at = text.indexOf(separator);
  if (at <= 0) return null;
  return [text.slice(0, at).trim(), text.slice(at + 1).trim()];
}

/** `claude mcp add [options] NAME COMMAND_OR_URL [ARGS]`, and `add-json NAME JSON`. */
function fromClaude(words: string[]): Pasted {
  if (words[0] === "add-json") {
    const [name, json] = words.slice(1);
    if (!name || !json) return { error: "claude mcp add-json needs a name and the JSON" };
    try {
      const value = parseLooseJson(json);
      if (!isRecord(value)) return { error: "That JSON isn't an MCP server" };
      const result = fromEntry(cleanName(name), value);
      return typeof result === "string" ? { error: result } : { found: [result] };
    } catch {
      return { error: "The JSON after claude mcp add-json doesn't parse" };
    }
  }
  let transport = "stdio";
  const env: Record<string, string> = {};
  const headers: Record<string, string> = {};
  const rest: string[] = [];
  let index = 1;
  for (; index < words.length; index++) {
    const word = words[index];
    const next = words[index + 1];
    if (word === "--") {
      rest.push(...words.slice(index + 1));
      break;
    } else if ((word === "--transport" || word === "-t") && next) {
      transport = next;
      index++;
    } else if ((word === "--env" || word === "-e") && next) {
      const pair = pairOf(next, "=");
      if (pair) env[pair[0]] = pair[1];
      index++;
    } else if ((word === "--header" || word === "-H") && next) {
      const pair = pairOf(next, ":");
      if (pair) headers[pair[0]] = pair[1];
      index++;
    } else if ((word === "--scope" || word === "-s" || word === "--client-id") && next) {
      index++;
    } else if (word.startsWith("--transport=")) {
      transport = word.slice("--transport=".length);
    } else if (word.startsWith("-") && rest.length < 2) {
      // Flags tectr has no use for, like --callback-port, are skipped.
    } else {
      rest.push(word);
    }
  }
  const [name, target, ...args] = rest;
  if (!name || !target) return { error: "claude mcp add needs a name and a command or url" };
  if (transport === "sse") return { error: `${name} uses SSE, which tectr doesn't support` };
  if (transport === "http" || isUrl(target)) {
    return { found: [found(cleanName(name), { type: "http", url: target, headers })] };
  }
  return {
    found: [found(cleanName(name), { type: "stdio", command: target, args, env })],
  };
}

/** `codex mcp add NAME [--env K=V] -- COMMAND [ARGS]`, or `codex mcp add NAME --url URL`. */
function fromCodex(words: string[]): Pasted {
  const name = words[1];
  if (!name) return { error: "codex mcp add needs a name" };
  const env: Record<string, string> = {};
  const headers: Record<string, string> = {};
  let url: string | undefined;
  const command: string[] = [];
  for (let index = 2; index < words.length; index++) {
    const word = words[index];
    const next = words[index + 1];
    if (word === "--") {
      command.push(...words.slice(index + 1));
      break;
    } else if (word === "--url" && next) {
      url = next;
      index++;
    } else if (word === "--env" && next) {
      const pair = pairOf(next, "=");
      if (pair) env[pair[0]] = pair[1];
      index++;
    } else if (word === "--bearer-token-env-var" && next) {
      headers.Authorization = `Bearer \${${next}}`;
      index++;
    } else if (!word.startsWith("-")) {
      command.push(...words.slice(index));
      break;
    }
  }
  if (url) return { found: [found(cleanName(name), { type: "http", url, headers })] };
  const [program, ...args] = command;
  if (!program) return { error: "codex mcp add needs a command after -- or a --url" };
  return { found: [found(cleanName(name), { type: "stdio", command: program, args, env })] };
}

/** What was pasted, as servers to add. */
export function readPasted(text: string): Pasted {
  const trimmed = text.trim();
  if (trimmed === "") return { empty: true };
  if (trimmed.startsWith("{") || trimmed.startsWith('"')) {
    try {
      return fromJson(parseLooseJson(trimmed.startsWith("{") ? trimmed : `{${trimmed}}`));
    } catch {
      return { error: "That looks like JSON but doesn't parse yet" };
    }
  }
  if (isUrl(trimmed)) {
    return { found: [found(nameFromUrl(trimmed), { type: "http", url: trimmed, headers: {} })] };
  }
  // A command copied from a README often goes on over several lines.
  const split = splitArgs(trimmed.replace(/\\\r?\n/g, " "));
  if ("error" in split) return { error: split.error };
  const words = split.args;
  if (words[0] === "claude" && words[1] === "mcp" && words[2]?.startsWith("add")) {
    return fromClaude(words.slice(2));
  }
  if (words[0] === "codex" && words[1] === "mcp" && words[2] === "add") {
    return fromCodex(words.slice(2));
  }
  // `KEY=value npx ...`, the way a shell passes environment to a command.
  const env: Record<string, string> = {};
  let start = 0;
  while (start < words.length && /^[A-Za-z_][A-Za-z0-9_]*=/.test(words[start])) {
    const pair = pairOf(words[start], "=");
    if (pair) env[pair[0]] = pair[1];
    start++;
  }
  const [command, ...args] = words.slice(start);
  if (!command) return { error: "There's no command to run" };
  return { found: [found(nameFromCommand(command, args), { type: "stdio", command, args, env })] };
}

/** A name not taken yet, by adding a number to it the way a file manager does. */
export function freeName(name: string, taken: Set<string>): string {
  if (!taken.has(name)) return name;
  for (let number = 2; ; number++) {
    const candidate = `${name}-${number}`;
    if (!taken.has(candidate)) return candidate;
  }
}

/** The transport with what was typed for each blank. A blank env variable left empty becomes
 * `${KEY}`, so the daemon reads it from its environment, which is where secrets belong. */
export function filledIn(server: Found, values: Record<string, string>): McpTransport {
  const fill = (where: Blank["in"], record: Record<string, string> | undefined) => {
    const out = { ...(record ?? {}) };
    for (const blank of server.blanks) {
      if (blank.in !== where) continue;
      const typed = values[blankId(blank)]?.trim() ?? "";
      out[blank.key] = typed !== "" ? typed : where === "env" ? `\${${blank.key}}` : out[blank.key];
    }
    return out;
  };
  const transport = server.transport;
  if (transport.type === "stdio") return { ...transport, env: fill("env", transport.env) };
  if (transport.type === "http") return { ...transport, headers: fill("headers", transport.headers) };
  return transport;
}

export const blankId = (blank: Blank) => `${blank.in}:${blank.key}`;

/** Headers can't fall back to the environment the way env variables can, so each blank one
 * has to be typed. */
export function blanksReady(server: Found, values: Record<string, string>): boolean {
  return server.blanks.every(
    (blank) => blank.in === "env" || (values[blankId(blank)]?.trim() ?? "") !== "",
  );
}

/** The names a new server can't have, as the daemon checks them: a device-wide name has to be
 * unique on the device, and a topic's own among the device's and that topic's. */
export function takenNames(servers: McpServer[], topicId?: string): Set<string> {
  return new Set(
    servers
      .filter((server) => !topicId || !server.topic_id || server.topic_id === topicId)
      .map((server) => server.name),
  );
}
