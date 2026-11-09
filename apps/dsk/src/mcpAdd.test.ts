import assert from "node:assert/strict";
import { test } from "node:test";
import {
  blanksReady,
  filledIn,
  freeName,
  isBlank,
  nameFromCommand,
  nameFromUrl,
  readPasted,
  takenNames,
} from "./mcp.ts";
import type { McpServer } from "./engine";

const one = (text: string) => {
  const pasted = readPasted(text);
  assert.ok("found" in pasted, JSON.stringify(pasted));
  assert.equal(pasted.found.length, 1);
  return pasted.found[0];
};

const problem = (text: string) => {
  const pasted = readPasted(text);
  assert.ok("error" in pasted, JSON.stringify(pasted));
  return pasted.error;
};

test("an address is an HTTP server named after its host", () => {
  assert.deepEqual(one("  https://mcp.linear.app/mcp \n"), {
    name: "linear",
    transport: { type: "http", url: "https://mcp.linear.app/mcp", headers: {} },
    blanks: [],
  });
  assert.equal(nameFromUrl("https://api.githubcopilot.com/mcp/"), "githubcopilot");
  assert.equal(nameFromUrl("https://mcp.sentry.dev/mcp"), "sentry");
  assert.equal(nameFromUrl("http://localhost:3000/mcp"), "localhost");
});

test("a command is a stdio server named after its package", () => {
  assert.deepEqual(one("npx -y @modelcontextprotocol/server-github"), {
    name: "github",
    transport: {
      type: "stdio",
      command: "npx",
      args: ["-y", "@modelcontextprotocol/server-github"],
      env: {},
    },
    blanks: [],
  });
  assert.equal(nameFromCommand("npx", ["@playwright/mcp@latest"]), "playwright");
  assert.equal(nameFromCommand("uvx", ["mcp-server-fetch"]), "fetch");
  assert.equal(nameFromCommand("npx", ["-y", "@upstash/context7-mcp"]), "context7");
  assert.equal(nameFromCommand("/usr/local/bin/my-tool", []), "my-tool");
});

test("variables before a command become its environment, and placeholders are blanks", () => {
  const server = one("GITHUB_TOKEN=<your-token> DEBUG=1 npx -y @modelcontextprotocol/server-github");
  assert.deepEqual(server.transport, {
    type: "stdio",
    command: "npx",
    args: ["-y", "@modelcontextprotocol/server-github"],
    env: { GITHUB_TOKEN: "<your-token>", DEBUG: "1" },
  });
  assert.deepEqual(server.blanks, [{ in: "env", key: "GITHUB_TOKEN" }]);
});

test("a claude mcp add line keeps its name, transport, env and headers", () => {
  assert.deepEqual(
    one("claude mcp add --transport http linear https://mcp.linear.app/mcp").transport,
    { type: "http", url: "https://mcp.linear.app/mcp", headers: {} },
  );
  const stdio = one(
    "claude mcp add github -s user -e GITHUB_TOKEN=YOUR_TOKEN -- npx -y @modelcontextprotocol/server-github",
  );
  assert.equal(stdio.name, "github");
  assert.deepEqual(stdio.transport, {
    type: "stdio",
    command: "npx",
    args: ["-y", "@modelcontextprotocol/server-github"],
    env: { GITHUB_TOKEN: "YOUR_TOKEN" },
  });
  assert.deepEqual(stdio.blanks, [{ in: "env", key: "GITHUB_TOKEN" }]);
  const headed = one(
    'claude mcp add --transport http sentry https://mcp.sentry.dev/mcp --header "Authorization: Bearer <token>"',
  );
  assert.deepEqual(headed.blanks, [{ in: "headers", key: "Authorization" }]);
  assert.match(problem("claude mcp add --transport sse old https://x"), /SSE/);
  assert.match(problem("claude mcp add"), /needs a name/);
});

test("a command over several lines reads as one", () => {
  const server = one("claude mcp add context7 \\\n  -- npx -y @upstash/context7-mcp");
  assert.equal(server.name, "context7");
});

test("claude mcp add-json and codex mcp add lines are read too", () => {
  assert.deepEqual(
    one(`claude mcp add-json weather '{"type":"stdio","command":"weather-mcp","args":["--metric"]}'`)
      .transport,
    { type: "stdio", command: "weather-mcp", args: ["--metric"], env: {} },
  );
  assert.deepEqual(one("codex mcp add docs --env KEY=1 -- docs-mcp --fast"), {
    name: "docs",
    transport: { type: "stdio", command: "docs-mcp", args: ["--fast"], env: { KEY: "1" } },
    blanks: [],
  });
  assert.deepEqual(one("codex mcp add sentry --url https://s.example/mcp --bearer-token-env-var TOKEN").transport, {
    type: "http",
    url: "https://s.example/mcp",
    headers: { Authorization: "Bearer ${TOKEN}" },
  });
});

test("JSON from docs is read in every common shape, comments and all", () => {
  const pasted = readPasted(`{
    // From the README
    "mcpServers": {
      "github": { "command": "npx", "args": ["-y", "gh"], "env": { "TOKEN": "" } },
      "Linear App": { "url": "https://mcp.linear.app/mcp" },
    },
  }`);
  assert.ok("found" in pasted);
  assert.deepEqual(
    pasted.found.map((f) => [f.name, f.transport.type, f.blanks.length]),
    [
      ["github", "stdio", 1],
      ["linear-app", "http", 0],
    ],
  );
  assert.equal(one(`"fetch": { "command": "uvx", "args": ["mcp-server-fetch"] }`).name, "fetch");
  assert.equal(one(`{ "command": "uvx", "args": ["mcp-server-fetch"] }`).name, "fetch");
  assert.deepEqual(
    one(`{"mcp": {"local": {"type": "local", "command": ["run", "it"], "environment": {"A": "b"}}}}`)
      .transport,
    { type: "stdio", command: "run", args: ["it"], env: { A: "b" } },
  );
  assert.equal(one(`{"servers": {"v": {"type": "http", "url": "https://v.example"}}}`).name, "v");
  assert.match(problem(`{"mcpServers": {"old": {"type": "sse", "url": "https://o"}}}`), /SSE/);
  assert.match(problem(`{"mcpServers": {`), /doesn't parse/);
});

test("empty input is neither a server nor a problem", () => {
  assert.deepEqual(readPasted("  \n "), { empty: true });
});

test("placeholders look the way docs write them, and variables are real values", () => {
  for (const value of ["", "<token>", "YOUR_API_KEY", "your-key-here", "xxx", "sk-..."]) {
    assert.ok(isBlank(value), value);
  }
  for (const value of ["${GITHUB_TOKEN}", "1", "https://x", "Bearer abc"]) {
    assert.ok(!isBlank(value), value);
  }
});

test("a blank env variable left empty reads from the daemon's environment", () => {
  const server = one(
    'claude mcp add -e TOKEN=<t> -e OTHER=1 --header "X-Key: <k>" s -- run',
  );
  assert.deepEqual(filledIn(server, {}), {
    type: "stdio",
    command: "run",
    args: [],
    env: { TOKEN: "${TOKEN}", OTHER: "1" },
  });
  assert.deepEqual(filledIn(server, { "env:TOKEN": " abc " }), {
    type: "stdio",
    command: "run",
    args: [],
    env: { TOKEN: "abc", OTHER: "1" },
  });
  const http = one('claude mcp add --transport http h https://h.example --header "X-Key: <k>"');
  assert.ok(!blanksReady(http, {}), "a header has nothing to fall back to");
  assert.ok(blanksReady(http, { "headers:X-Key": "secret" }));
  assert.deepEqual(filledIn(http, { "headers:X-Key": "secret" }), {
    type: "http",
    url: "https://h.example",
    headers: { "X-Key": "secret" },
  });
});

test("a taken name gets a number", () => {
  assert.equal(freeName("github", new Set()), "github");
  assert.equal(freeName("github", new Set(["github", "github-2"])), "github-3");
});

test("a topic can reuse a name only another topic has", () => {
  const server = (name: string, topic_id?: string): McpServer => ({
    id: name,
    name,
    ...(topic_id && { topic_id }),
    transport: { type: "stdio", command: "x" },
    enabled: true,
  });
  const servers = [server("device"), server("mine", "t1"), server("theirs", "t2")];
  assert.deepEqual([...takenNames(servers)].sort(), ["device", "mine", "theirs"]);
  assert.deepEqual([...takenNames(servers, "t1")].sort(), ["device", "mine"]);
});
