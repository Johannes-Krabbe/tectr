import assert from "node:assert/strict";
import { test } from "node:test";
import type { DaemonState, McpProfile, McpServer, Workspace } from "./engine";
import {
  addableServers,
  authLabel,
  canEdit,
  canHaveProfiles,
  canLogOut,
  canRemove,
  connectorAgent,
  connectorSummary,
  connectors,
  copiedMcp,
  draftOf,
  draftReady,
  entryAuth,
  formatPairs,
  importSummary,
  joinArgs,
  leftOut,
  libraryServers,
  matchesServer,
  mcpSummary,
  nameProblem,
  newEntry,
  nextProfile,
  parsePairs,
  profileLabel,
  profileNameProblem,
  splitArgs,
  takenNames,
  transportOf,
  usedBy,
  valuesLabel,
  withServer,
  workspaceServers,
} from "./mcp.ts";

const profile = (id: string, auth?: McpProfile["auth"]): McpProfile => ({
  id,
  name: id,
  ...(auth && { auth }),
});

const server = (id: string, profiles: McpProfile[] = []): McpServer => ({
  id,
  name: id,
  transport: { type: "stdio", command: "x" },
  ...(profiles.length > 0 && { profiles }),
});

const http = (id: string, profiles: McpProfile[] = [], needs_login = false): McpServer => ({
  ...server(id, profiles),
  transport: { type: "http", url: `https://${id}.example/mcp` },
  ...(needs_login && { needs_login }),
});

const slack: McpServer = {
  id: "c1",
  name: "claude.ai Slack",
  transport: { type: "claude_connector", url: "https://mcp.slack.com/mcp" },
};

const workspace = (id: string, mcp: Workspace["mcp"] = {}): Workspace => ({
  id,
  name: id,
  path: `/${id}`,
  topics: [],
  mcp,
});

const notion = http("notion", [profile("personal", "logged_in"), profile("work", "needs_login")], true);

const state: DaemonState = {
  machine: { id: "m", name: "m" },
  workspaces: [
    workspace("tectr", { notion: { enabled: true, profile_id: "work" }, files: { enabled: false } }),
    workspace("blog", { notion: { enabled: true, profile_id: "personal" } }),
  ],
  approvals: [],
  mcp_servers: [server("files"), notion, server("github"), slack],
};

const names = (servers: McpServer[]) => servers.map((s) => s.name);

test("the library lists servers and connectors apart", () => {
  assert.deepEqual(names(libraryServers(state)), ["files", "notion", "github"]);
  assert.deepEqual(names(connectors(state)), ["claude.ai Slack"]);
  assert.deepEqual(names(libraryServers({ ...state, mcp_servers: undefined })), []);
});

// The same cases as the protocol's `a_workspace_gets_the_servers_it_turns_on_with_their_profiles`,
// plus the ones turned off, which the page still lists.
test("a workspace lists its servers in the library's order, with their profiles", () => {
  const [tectr] = state.workspaces;
  const listed = workspaceServers(state, tectr).map(({ server, entry, profile }) => [
    server.name,
    entry.enabled,
    profile?.name,
  ]);
  assert.deepEqual(listed, [
    ["files", false, undefined],
    ["notion", true, "work"],
  ]);
  const gone = workspace("w", { notion: { enabled: true, profile_id: "deleted" } });
  assert.equal(workspaceServers(state, gone)[0].profile, undefined);
});

test("adding offers what the workspace doesn't have yet, connectors last", () => {
  assert.deepEqual(names(addableServers(state, state.workspaces[0])), ["github", "claude.ai Slack"]);
  assert.deepEqual(names(addableServers(state, workspace("empty"))), [
    "files",
    "notion",
    "github",
    "claude.ai Slack",
  ]);
});

test("a server starts on, with its profile when it has only one", () => {
  assert.deepEqual(newEntry(server("a")), { enabled: true });
  assert.deepEqual(newEntry(server("a", [profile("p")])), { enabled: true, profile_id: "p" });
  assert.deepEqual(newEntry(notion), { enabled: true });
});

test("a workspace's list changes one server at a time", () => {
  const list = { a: { enabled: true } };
  assert.deepEqual(withServer(list, "b", { enabled: false }), {
    a: { enabled: true },
    b: { enabled: false },
  });
  assert.deepEqual(withServer(list, "a", null), {});
  assert.deepEqual(withServer(undefined, "a", { enabled: true }), { a: { enabled: true } });
});

test("copying adds the other workspace's servers this one lacks, as they are there", () => {
  const library = state.mcp_servers ?? [];
  const target = { notion: { enabled: true, profile_id: "personal" } };
  const source = {
    notion: { enabled: true, profile_id: "work" },
    github: { enabled: false },
    removed: { enabled: true },
  };
  assert.deepEqual(copiedMcp(target, source, library), {
    notion: { enabled: true, profile_id: "personal" },
    github: { enabled: false },
  });
  assert.deepEqual(copiedMcp(undefined, undefined, library), {});
});

test("switching profiles goes through each, then none, then around", () => {
  assert.equal(nextProfile(notion, undefined), "personal");
  assert.equal(nextProfile(notion, "personal"), "work");
  assert.equal(nextProfile(notion, "work"), null);
  assert.equal(nextProfile(notion, "deleted"), null);
  assert.equal(nextProfile(server("a"), null), null);
});

test("a profile shows by name, and a missing one as none", () => {
  assert.equal(profileLabel(notion, "work"), "work");
  assert.equal(profileLabel(notion, undefined), "No profile");
  assert.equal(profileLabel(notion, "deleted"), "No profile");
});

test("a server that wants a login needs a profile that has one", () => {
  const [personal, work] = notion.profiles ?? [];
  assert.equal(entryAuth(notion, personal), "logged_in");
  assert.equal(entryAuth(notion, work), "needs_login");
  assert.equal(entryAuth(notion, undefined), "needs_login");
  assert.equal(entryAuth(http("open"), undefined), null);
  assert.equal(entryAuth(server("s", [profile("p", "needs_login")]), undefined), null);
  assert.equal(authLabel("needs_login"), "Needs login");
  assert.equal(authLabel("logged_in"), "Logged in");
  assert.equal(authLabel(null), null);
  assert.equal(entryAuth(http("open"), profile("p")), null);
});

test("the library says which workspaces use a server or one of its profiles", () => {
  assert.deepEqual(usedBy(state, "notion"), ["tectr", "blog"]);
  assert.deepEqual(usedBy(state, "notion", "work"), ["tectr"]);
  assert.deepEqual(usedBy(state, "github"), []);
});

test("a profile's name is needed, and only once per server", () => {
  assert.equal(profileNameProblem(" ", notion), "A profile needs a name");
  assert.equal(profileNameProblem("work", notion), "notion already has a profile called work");
  assert.equal(profileNameProblem("work", notion, "work"), null);
  assert.equal(profileNameProblem("client", notion), null);
  assert.equal(valuesLabel(notion), "Headers");
  assert.equal(valuesLabel(server("s")), "Environment");
});

test("searching matches every word against the name and what it runs", () => {
  assert.ok(matchesServer(notion, "not"));
  assert.ok(matchesServer(notion, "notion example"));
  assert.ok(!matchesServer(notion, "notion github"));
  assert.ok(matchesServer(server("files"), ""));
});

test("connectors are their agent's: no profiles, no editing, no removing", () => {
  const app: McpServer = { id: "c2", name: "ChatGPT Slack", transport: { type: "codex_app", id: "a" } };
  assert.equal(connectorAgent(slack), "Claude");
  assert.equal(connectorAgent(app), "Codex");
  assert.equal(connectorAgent(server("x")), null);
  assert.equal(mcpSummary(slack.transport), "https://mcp.slack.com/mcp");
  assert.ok(!canEdit(slack));
  assert.ok(!canRemove(app));
  assert.ok(!canHaveProfiles(slack));
  assert.ok(canEdit(server("x")));
  assert.ok(canHaveProfiles(server("x")));
  assert.ok(!canEdit({ ...server("x"), transport: { type: "unknown" } }));
});

test("only a logged in profile of an http server can be logged out of", () => {
  const [personal, work] = notion.profiles ?? [];
  assert.equal(canLogOut(notion, personal), true);
  assert.equal(canLogOut(notion, work), false);
  assert.equal(canLogOut(notion, undefined), false);
  assert.equal(canLogOut(server("s"), profile("p", "logged_in")), false);
});

test("every name on a device is taken once", () => {
  assert.deepEqual([...takenNames(state.mcp_servers ?? [])].sort(), [
    "claude.ai Slack",
    "files",
    "github",
    "notion",
  ]);
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
  };
  for (const s of [stdio, http]) {
    const draft = draftOf(s);
    assert.ok(draftReady(draft));
    assert.deepEqual(transportOf(draft), { transport: s.transport });
  }
});

test("a new server leaves out what's empty", () => {
  const draft = draftOf(undefined);
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
