import assert from "node:assert/strict";
import { test } from "node:test";
import type { DaemonState, McpServer, Topic } from "./engine";
import {
  canEdit,
  canLogOut,
  canRemove,
  canToggle,
  connectorAgent,
  connectorSummary,
  connectors,
  deviceServers,
  draftOf,
  draftReady,
  flippedTopicMcp,
  formatPairs,
  importSummary,
  joinArgs,
  leftOut,
  mcpAuthLabel,
  mcpSummary,
  nameProblem,
  ownServers,
  parsePairs,
  splitArgs,
  topicMcpChoice,
  topicMcpLabel,
  topicMcpServers,
  transportOf,
  withEnabled,
} from "./mcp.ts";

const server = (id: string, topic_id: string | undefined, enabled: boolean): McpServer => ({
  id,
  name: id,
  topic_id,
  transport: { type: "stdio", command: "x" },
  enabled,
});

const topic = (id: string, mcp: Record<string, boolean> = {}): Topic => ({
  id,
  name: id,
  worktrees: [],
  sessions: [],
  mcp,
});

const state: DaemonState = {
  machine: { id: "m", name: "m" },
  workspaces: [],
  approvals: [],
  mcp_servers: [
    server("on", undefined, true),
    server("off", undefined, false),
    server("mine", "t", true),
    server("mine-off", "t", false),
    server("theirs", "other", true),
  ],
};

const names = (servers: McpServer[]) => servers.map((s) => s.name);

test("device servers are the ones without a topic, and a topic's own are only its", () => {
  assert.deepEqual(names(deviceServers(state)), ["on", "off"]);
  assert.deepEqual(names(ownServers(state, "t")), ["mine", "mine-off"]);
});

// The same cases as the protocol's `a_topic_gets_the_device_servers_it_leaves_on_and_its_own`.
test("a topic gets the device servers it leaves on and its own that are on", () => {
  assert.deepEqual(names(topicMcpServers(state, topic("t"))), ["on", "mine"]);
  assert.deepEqual(names(topicMcpServers(state, topic("t", { on: false, off: true }))), [
    "off",
    "mine",
  ]);
  assert.deepEqual(names(topicMcpServers(state, topic("other"))), ["on", "theirs"]);
  assert.deepEqual(names(topicMcpServers({ ...state, mcp_servers: undefined }, topic("t"))), []);
});

test("connectors are listed apart, only turned on or off, and reach a topic like device servers", () => {
  const slack: McpServer = {
    id: "c1",
    name: "claude.ai Slack",
    transport: { type: "claude_connector", url: "https://mcp.slack.com/mcp" },
    enabled: false,
  };
  const app: McpServer = {
    id: "c2",
    name: "ChatGPT Slack",
    transport: { type: "codex_app", id: "asdk_app_1" },
    enabled: true,
  };
  const withConnectors = { ...state, mcp_servers: [...(state.mcp_servers ?? []), slack, app] };
  assert.deepEqual(names(deviceServers(withConnectors)), ["on", "off"]);
  assert.deepEqual(names(connectors(withConnectors)), ["claude.ai Slack", "ChatGPT Slack"]);
  assert.deepEqual(names(topicMcpServers(withConnectors, topic("t", { c1: true }))), [
    "on",
    "mine",
    "claude.ai Slack",
    "ChatGPT Slack",
  ]);
  assert.equal(connectorAgent(slack), "Claude");
  assert.equal(connectorAgent(app), "Codex");
  assert.equal(connectorAgent(server("x", undefined, true)), null);
  assert.equal(mcpSummary(slack.transport), "https://mcp.slack.com/mcp");
  assert.ok(canToggle(slack));
  assert.ok(!canEdit(slack));
  assert.ok(!canRemove(app));
  assert.ok(canEdit(server("x", undefined, true)));
  assert.deepEqual(withEnabled(slack, true), {
    command: "save_mcp_server",
    id: "c1",
    name: "claude.ai Slack",
    transport: slack.transport,
    enabled: true,
  });
});

test("refreshing connectors says how many changed, not which", () => {
  const some = ["claude.ai Notion", "claude.ai Linear", "claude.ai Slack"];
  assert.equal(connectorSummary(some, [], []), "Added 3 connectors");
  assert.equal(connectorSummary(["claude.ai Notion"], [], []), "Added 1 connector");
  assert.equal(connectorSummary(some, ["claude.ai Gmail"], []), "Added 3 connectors, removed 1");
  assert.equal(connectorSummary([], some.slice(1), []), "Removed 2 connectors");
  assert.equal(connectorSummary([], [], []), "No new connectors");
  // What was added is still worth saying next to what couldn't be listed.
  assert.equal(connectorSummary(some, [], ["no list"]), "Added 3 connectors");
});

test("what couldn't be listed stays on screen instead of in a toast", () => {
  assert.equal(connectorSummary([], [], ["x"]), null);
  assert.equal(leftOut(["x", "y"], "Couldn't list"), "Couldn't list: x\nCouldn't list: y");
  assert.equal(leftOut([], "Couldn't list"), null);
});

test("a topic follows a device server's default until it turns it on or off", () => {
  const [on, off] = deviceServers(state);
  assert.equal(topicMcpChoice(topic("t"), on), "default");
  assert.equal(topicMcpLabel(topic("t"), on), "On by default");
  assert.equal(topicMcpLabel(topic("t"), off), "Off by default");
  assert.equal(topicMcpChoice(topic("t", { on: true }), on), "on");
  assert.equal(topicMcpLabel(topic("t", { on: true }), on), "On here");
  assert.equal(topicMcpLabel(topic("t", { off: false }), off), "Off here");
});

test("flipping a device server for a topic goes back to the default when it lands there", () => {
  const [on, off] = deviceServers(state);
  assert.equal(flippedTopicMcp(topic("t"), on), false);
  assert.equal(flippedTopicMcp(topic("t", { on: false }), on), null);
  assert.equal(flippedTopicMcp(topic("t"), off), true);
  assert.equal(flippedTopicMcp(topic("t", { off: true }), off), null);
  // Turned on while the default is on too: flipping turns it off for this topic.
  assert.equal(flippedTopicMcp(topic("t", { on: true }), on), false);
});

test("arguments split like a shell's and join back to the same ones", () => {
  assert.deepEqual(splitArgs("-y  @scope/pkg"), { args: ["-y", "@scope/pkg"] });
  assert.deepEqual(splitArgs(`--name "two words" 'it"s' a\\ b ""`), {
    args: ["--name", "two words", 'it"s', "a b", ""],
  });
  assert.deepEqual(splitArgs(`"a\\"b" 'c\\d'`), { args: ['a"b', "c\\d"] });
  assert.deepEqual(splitArgs("   "), { args: [] });
  assert.ok("error" in splitArgs(`"open`));
  const args = ["plain", "two words", 'say "hi"', "back\\slash", "", "${TOKEN}"];
  assert.deepEqual(splitArgs(joinArgs(args)), { args });
  assert.equal(joinArgs(["-y", "pkg"]), "-y pkg");
});

test("pairs are one KEY=VALUE a line, and say which line is wrong", () => {
  assert.deepEqual(parsePairs("A=1\n\n B = two words \nC=x=y\nD=", "Env"), {
    pairs: { A: "1", B: "two words", C: "x=y", D: "" },
  });
  assert.deepEqual(parsePairs("A=1\nnope", "Env"), { error: "Env: line 2 needs KEY=VALUE" });
  assert.deepEqual(parsePairs("=1", "Env"), { error: "Env: line 1 needs KEY=VALUE" });
  assert.deepEqual(parsePairs("A=1\nA=2", "Env"), { error: "Env: A is there twice" });
  assert.equal(formatPairs({ A: "1", TOKEN: "${GITHUB_TOKEN}" }), "A=1\nTOKEN=${GITHUB_TOKEN}");
  assert.equal(formatPairs(undefined), "");
});

test("a summary is the command line for stdio and the address for http", () => {
  assert.equal(
    mcpSummary({ type: "stdio", command: "npx", args: ["-y", "gh mcp"] }),
    'npx -y "gh mcp"',
  );
  assert.equal(mcpSummary({ type: "http", url: "https://mcp.linear.app/mcp" }), "https://mcp.linear.app/mcp");
});

test("only http servers show a login, and only a held one can be logged out of", () => {
  const http: McpServer = { ...server("h", undefined, true), transport: { type: "http", url: "u" } };
  assert.equal(mcpAuthLabel(http), null);
  assert.equal(mcpAuthLabel({ ...http, auth: "needs_login" }), "Needs login");
  assert.equal(mcpAuthLabel({ ...http, auth: "logged_in" }), "Logged in");
  assert.equal(mcpAuthLabel({ ...server("s", undefined, true), auth: "logged_in" }), null);
  assert.equal(canLogOut(http), false);
  assert.equal(canLogOut({ ...http, auth: "logged_in" }), true);
});

test("names use letters, digits, dashes and underscores", () => {
  assert.equal(nameProblem(""), null);
  assert.equal(nameProblem("git-hub_2"), null);
  assert.equal(nameProblem("git hub"), "Names use letters, digits, - and _");
});

test("a server goes into the form and back out unchanged", () => {
  const stdio: McpServer = {
    id: "x",
    name: "github",
    transport: { type: "stdio", command: "npx", args: ["-y", "gh-mcp"], env: { TOKEN: "${GITHUB_TOKEN}" } },
    enabled: false,
  };
  const http: McpServer = {
    id: "y",
    name: "linear",
    transport: {
      type: "http",
      url: "https://mcp.linear.app/mcp",
      headers: { "X-Team": "core" },
      oauth_client_id: "abc",
    },
    enabled: true,
  };
  for (const s of [stdio, http]) {
    const draft = draftOf(s);
    assert.equal(draft.enabled, s.enabled);
    assert.ok(draftReady(draft));
    assert.deepEqual(transportOf(draft), { transport: s.transport });
  }
});

test("a new server starts on, and leaves out what's empty", () => {
  const draft = draftOf(undefined);
  assert.equal(draft.enabled, true);
  assert.equal(draftReady(draft), false);
  assert.equal(draftReady({ ...draft, name: "a b", command: "x" }), false);
  assert.equal(draftReady({ ...draft, name: "ok", command: " " }), false);
  assert.ok(draftReady({ ...draft, name: "ok", command: "x" }));
  assert.ok(draftReady({ ...draft, name: "ok", type: "http", url: "https://x" }));
  assert.deepEqual(transportOf({ ...draft, command: " uvx " }), {
    transport: { type: "stdio", command: "uvx" },
  });
  assert.deepEqual(transportOf({ ...draft, type: "http", url: "https://x", oauthClientId: " " }), {
    transport: { type: "http", url: "https://x" },
  });
  assert.deepEqual(transportOf({ ...draft, command: "x", env: "oops" }), {
    error: "Environment: line 1 needs KEY=VALUE",
  });
});

test("the import says how many were added, and why the rest wasn't stays on screen", () => {
  const skipped = ["old: sse servers aren't supported"];
  assert.equal(importSummary(["github", "linear"], []), "Added 2 servers");
  assert.equal(importSummary(["github"], skipped), "Added 1 server, 1 skipped");
  assert.equal(importSummary([], skipped), "Nothing added, 1 skipped");
  assert.equal(importSummary([], []), "Nothing new to import");
  assert.equal(leftOut(skipped, "Skipped"), "Skipped old: sse servers aren't supported");
});

test("turning a server on or off sends it back whole, keeping its topic", () => {
  const own: McpServer = { ...server("mine", "t", true), transport: { type: "stdio", command: "x", args: ["a"] } };
  assert.deepEqual(withEnabled(own, false), {
    command: "save_mcp_server",
    id: "mine",
    name: "mine",
    topic_id: "t",
    transport: { type: "stdio", command: "x", args: ["a"] },
    enabled: false,
  });
  assert.equal("topic_id" in (withEnabled(server("d", undefined, false), true) ?? {}), false);
  assert.equal(withEnabled({ ...own, transport: { type: "unknown" } }, false), null);
});
