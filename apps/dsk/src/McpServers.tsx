import { invoke } from "@tauri-apps/api/core";
import { type KeyboardEvent, useEffect, useState } from "react";
import {
  type Command,
  type CommandOutput,
  type Device,
  errorText,
  type McpServer,
  request,
} from "./engine";
import { act, type ActionId, keysLabel } from "./keybinds";
import { inPane } from "./keys";
import {
  canEdit,
  canLogIn,
  canLogOut,
  deviceServers,
  draftOf,
  draftReady,
  importLines,
  mcpAuthLabel,
  type McpDraft,
  mcpSummary,
  nameProblem,
  transportOf,
  withEnabled,
} from "./mcp";
import { isOnline } from "./model";
import { type PaletteItem, useScreenPaletteItems } from "./Palette";
import { Sheet } from "./Sheets";
import { hint, type Hint, useHints } from "./shortcuts";
import { Keys } from "./ui/StatusIcon";

/** Requests about servers, each one's progress and last error kept by server. */
export function useMcpRequests(machineId: string | undefined) {
  const [busy, setBusy] = useState<Record<string, string>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});

  const fail = (id: string, reason: unknown) =>
    setErrors((all) => ({ ...all, [id]: errorText(reason) }));

  const run = async (id: string, doing: string, command: Command) => {
    if (!machineId || busy[id]) return undefined;
    setBusy((all) => ({ ...all, [id]: doing }));
    setErrors(({ [id]: _cleared, ...rest }) => rest);
    try {
      return await request(machineId, command);
    } catch (reason) {
      fail(id, reason);
      return undefined;
    } finally {
      setBusy(({ [id]: _done, ...rest }) => rest);
    }
  };

  const toggle = (server: McpServer) => {
    const command = withEnabled(server, !server.enabled);
    if (command) void run(server.id, "Saving", command);
  };

  const remove = (server: McpServer) =>
    void run(server.id, "Removing", { command: "remove_mcp_server", id: server.id });

  // The browser comes back to the daemon, and the state says logged in once it has.
  const logIn = async (server: McpServer) => {
    if (!canLogIn(server)) return;
    const output = await run(server.id, "Starting login", {
      command: "login_mcp_server",
      id: server.id,
    });
    if (output?.output !== "mcp_login") return;
    invoke("open_url", { url: output.url }).catch((reason) => fail(server.id, reason));
  };

  const logOut = (server: McpServer) => {
    if (canLogOut(server)) {
      void run(server.id, "Logging out", { command: "logout_mcp_server", id: server.id });
    }
  };

  return { busy, errors, run, toggle, remove, logIn: (s: McpServer) => void logIn(s), logOut };
}

export type McpRequests = ReturnType<typeof useMcpRequests>;

/** What one server's row can do, in the order its buttons show. */
export function serverActions(
  server: McpServer,
  requests: McpRequests,
  onEdit: (server: McpServer) => void,
  toggleLabel: string,
): { label: string; action: ActionId; run: () => void }[] {
  const actions: { label: string; action: ActionId; run: () => void }[] = [];
  if (canEdit(server)) {
    actions.push({ label: toggleLabel, action: "mcp.toggle", run: () => requests.toggle(server) });
    actions.push({ label: "Edit", action: "mcp.edit", run: () => onEdit(server) });
  }
  if (canLogIn(server)) {
    actions.push({ label: "Log in", action: "mcp.login", run: () => requests.logIn(server) });
  }
  if (canLogOut(server)) {
    actions.push({ label: "Log out", action: "mcp.logout", run: () => requests.logOut(server) });
  }
  actions.push({ label: "Remove", action: "mcp.remove", run: () => requests.remove(server) });
  return actions;
}

/** Runs whichever of a row's actions this press is the key for. */
export function actOnServer(
  event: globalThis.KeyboardEvent | KeyboardEvent,
  actions: { action: ActionId; run: () => void }[],
): boolean {
  return actions.some(({ action, run }) => act(event, action, run));
}

const TYPES: McpDraft["type"][] = ["stdio", "http"];

const FORM_HINTS: Hint[] = [
  hint("Save", "mcp_form.save"),
  hint("Stdio or HTTP", "mcp_form.switch_type"),
  hint("On, off", "mcp_form.toggle_enabled"),
];

/** Adds a server, or changes one when `server` is given. A topic's own server has `topicId`. */
export function McpServerSheet({
  machineId,
  server,
  topicId,
  where,
  onClose,
}: {
  machineId: string;
  server?: McpServer;
  topicId?: string;
  where: string;
  onClose: () => void;
}) {
  const [draft, setDraft] = useState<McpDraft>(() => draftOf(server));
  const change = (next: Partial<McpDraft>) => setDraft((current) => ({ ...current, ...next }));
  const switchType = () => change({ type: draft.type === "stdio" ? "http" : "stdio" });
  const toggleEnabled = () => change({ enabled: !draft.enabled });
  const ownerId = server ? server.topic_id : topicId;

  const save = async (): Promise<CommandOutput> => {
    const built = transportOf(draft);
    // Shown as is, like the daemon's own messages.
    if ("error" in built) throw built.error;
    return request(machineId, {
      command: "save_mcp_server",
      ...(server && { id: server.id }),
      name: draft.name.trim(),
      ...(ownerId && { topic_id: ownerId }),
      transport: built.transport,
      enabled: draft.enabled,
    });
  };

  // Enter adds a line in these, so saving from one takes the save key.
  const lines = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    const plain = !event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey;
    if (event.key === "Enter" && plain && !event.nativeEvent.isComposing) event.stopPropagation();
  };

  const field = (
    label: string,
    key: "command" | "args" | "url" | "oauthClientId",
    placeholder: string,
  ) => (
    <label className="sheet-field">
      <span className="sheet-label">{label}</span>
      <input
        className="sheet-input code"
        value={draft[key]}
        placeholder={placeholder}
        spellCheck={false}
        autoCorrect="off"
        autoCapitalize="off"
        onChange={(event) => change({ [key]: event.target.value })}
      />
    </label>
  );

  const pairs = (label: string, key: "env" | "headers") => (
    <label className="sheet-field">
      <span className="sheet-label">{label}</span>
      <textarea
        className="sheet-input code"
        value={draft[key]}
        placeholder="KEY=VALUE"
        rows={2}
        spellCheck={false}
        autoCorrect="off"
        autoCapitalize="off"
        onChange={(event) => change({ [key]: event.target.value })}
        onKeyDown={lines}
      />
    </label>
  );

  return (
    <Sheet
      where={where}
      action="Save"
      hint={nameProblem(draft.name.trim()) ?? "Values can use ${NAME} from the daemon's environment"}
      canSubmit={draftReady(draft)}
      returnsFocus
      run={save}
      onClose={onClose}
      onDone={() => {}}
      hints={FORM_HINTS}
      input={JSON.stringify(draft)}
      onKeyDown={(event) => {
        const form = event.currentTarget;
        if (act(event, "mcp_form.save", () => form.requestSubmit())) return;
        if (act(event, "mcp_form.switch_type", switchType)) return;
        act(event, "mcp_form.toggle_enabled", toggleEnabled);
      }}
    >
      <input
        className="sheet-name code"
        autoFocus
        value={draft.name}
        placeholder="name"
        aria-label="Server name"
        spellCheck={false}
        autoCorrect="off"
        autoCapitalize="off"
        onChange={(event) => change({ name: event.target.value })}
      />
      <div className="sheet-fields">
        <div className="sheet-field">
          <span className="sheet-label">Type</span>
          <span className="sheet-choices" role="group" aria-label="Type">
            {TYPES.map((type) => (
              <button
                key={type}
                type="button"
                className="picker"
                aria-pressed={draft.type === type}
                tabIndex={-1}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => change({ type })}
              >
                {type === "stdio" ? "Stdio" : "HTTP"}
              </button>
            ))}
            <Keys action="mcp_form.switch_type" />
          </span>
        </div>
        {draft.type === "stdio" ? (
          <>
            {field("Command", "command", "npx")}
            {field("Arguments", "args", "-y @scope/server")}
            {pairs("Environment", "env")}
          </>
        ) : (
          <>
            {field("URL", "url", "https://")}
            {pairs("Headers", "headers")}
            {field("OAuth client ID", "oauthClientId", "Only if the server can't register one")}
          </>
        )}
        <div className="sheet-field">
          <span className="sheet-label">{ownerId ? "On" : "On for topics by default"}</span>
          <span className="sheet-choices">
            <button
              type="button"
              className="picker"
              aria-pressed={draft.enabled}
              tabIndex={-1}
              onMouseDown={(event) => event.preventDefault()}
              onClick={toggleEnabled}
            >
              {draft.enabled ? "On" : "Off"}
            </button>
            <Keys action="mcp_form.toggle_enabled" />
          </span>
        </div>
      </div>
    </Sheet>
  );
}

type Form = { server?: McpServer } | null;

function paletteLabel(server: McpServer, action: ActionId): string {
  const { name } = server;
  switch (action) {
    case "mcp.toggle":
      return `Turn ${name} ${server.enabled ? "off" : "on"} for topics by default`;
    case "mcp.login":
      return `Log in to ${name}`;
    case "mcp.logout":
      return `Log out of ${name}`;
    case "mcp.remove":
      return `Remove ${name}`;
    default:
      return `Edit ${name}`;
  }
}

/** The device's own servers, which every topic gets unless it turns one off. */
export function McpSettings({
  device,
  active,
  onActivate,
  onLeave,
}: {
  device: Device;
  /** This section has the keyboard, rather than the list of sections. */
  active: boolean;
  onActivate: () => void;
  onLeave: () => void;
}) {
  const state = device.state;
  const machineId = state?.machine.id;
  const online = isOnline(device);
  const servers = state ? deviceServers(state) : [];
  const requests = useMcpRequests(machineId);
  const [cursor, setCursor] = useState(0);
  const [form, setForm] = useState<Form>(null);
  const [importing, setImporting] = useState(false);
  const [imported, setImported] = useState<string[] | null>(null);
  const [importError, setImportError] = useState<string | null>(null);

  const selected = Math.max(Math.min(cursor, servers.length - 1), 0);
  const server = servers[selected];
  const deviceName = state?.machine.name ?? device.url;

  const add = () => {
    if (online) setForm({});
  };
  const edit = (target: McpServer) => {
    if (online && canEdit(target)) setForm({ server: target });
  };

  const runImport = async () => {
    if (!machineId || !online || importing) return;
    setImporting(true);
    setImportError(null);
    try {
      const output = await request(machineId, { command: "import_mcp_servers" });
      if (output.output === "mcp_import") setImported(importLines(output.added, output.skipped));
    } catch (reason) {
      setImportError(errorText(reason));
    } finally {
      setImporting(false);
    }
  };

  const toggleLabel = (target: McpServer) => (target.enabled ? "Turn off" : "Turn on");
  const rowActions = (target: McpServer) =>
    online ? serverActions(target, requests, edit, toggleLabel(target)) : [];

  useHints(
    "pane",
    active
      ? [
          ...(online ? [hint("Add", "mcp.add"), hint("Import", "mcp.import")] : []),
          ...(server ? rowActions(server).map(({ label, action }) => hint(label, action)) : []),
          ...(servers.length > 1
            ? [hint("Move", "mcp.up", "mcp.down"), hint("First, last", "mcp.first", "mcp.last")]
            : []),
          hint("Back", "mcp.back"),
        ]
      : null,
  );

  // Running one from the palette lands in this section, where its key works.
  const paletteItems: PaletteItem[] = [];
  if (online) {
    const then = (run: () => void) => () => {
      onActivate();
      run();
    };
    paletteItems.push(
      {
        id: "mcp-add",
        label: `Add an MCP server on ${deviceName}`,
        keys: keysLabel("mcp.add"),
        run: then(add),
      },
      {
        id: "mcp-import",
        label: `Import MCP servers on ${deviceName}`,
        detail: "From Claude Code, Codex and OpenCode",
        keys: keysLabel("mcp.import"),
        run: then(() => void runImport()),
      },
    );
    for (const target of servers) {
      for (const { action, run } of rowActions(target)) {
        paletteItems.push({
          id: `mcp/${target.id}/${action}`,
          label: paletteLabel(target, action),
          detail: "MCP server",
          keys: target === server && active ? keysLabel(action) : undefined,
          run: then(run),
        });
      }
    }
  }
  useScreenPaletteItems(paletteItems);

  useEffect(() => {
    if (active) document.querySelector(".settings-item.current")?.scrollIntoView({ block: "nearest" });
  }, [selected, active]);

  useEffect(() => {
    if (!active || form) return;
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (!inPane(event)) return;
      const moveTo = (index: number) => () =>
        setCursor(Math.max(Math.min(index, servers.length - 1), 0));
      const commands: [ActionId, () => void][] = [
        ["mcp.up", moveTo(selected - 1)],
        ["mcp.down", moveTo(selected + 1)],
        ["mcp.first", moveTo(0)],
        ["mcp.last", moveTo(servers.length - 1)],
        [
          "mcp.center",
          () => document.querySelector(".settings-item.current")?.scrollIntoView({ block: "center" }),
        ],
        ["mcp.back", onLeave],
        ["mcp.add", add],
        ["mcp.import", () => void runImport()],
      ];
      if (commands.some(([id, run]) => act(event, id, run))) return;
      if (server) actOnServer(event, rowActions(server));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  return (
    <div className="pane settings">
      <div className="settings-in">
        <div className="settings-row">
          <span className="name">Topics get the ones that are on, and each topic can change that</span>
          <button className="button" disabled={!online || importing} onClick={() => void runImport()}>
            {importing ? "Importing" : "Import"}
            {active && <Keys action="mcp.import" />}
          </button>
          <button className="button" disabled={!online} onClick={add}>
            Add server
            {active && <Keys action="mcp.add" />}
          </button>
        </div>
        {!online && <p className="empty settings-error">This device is offline</p>}
        {importError && <p className="error settings-error">{importError}</p>}
        {imported?.map((line, index) => (
          <p key={index} className="settings-note settings-error">
            {line}
          </p>
        ))}
        {online && servers.length === 0 && (
          <p className="empty settings-error">No servers yet. Agents get none of their own.</p>
        )}
        {servers.map((target, index) => {
          const current = active && index === selected;
          const doing = requests.busy[target.id];
          const auth = mcpAuthLabel(target);
          return (
            <div
              key={target.id}
              className={`settings-item ${current ? "current" : ""}`}
              onMouseMove={(event) => {
                if (event.movementX === 0 && event.movementY === 0) return;
                setCursor(index);
                onActivate();
              }}
            >
              <div className="settings-row">
                <span className="name code">{target.name}</span>
                {doing && <span className="meta">{doing}</span>}
                {auth && (
                  <span className={`meta ${target.auth === "needs_login" ? "alert" : ""}`}>{auth}</span>
                )}
                <span className="meta">{target.enabled ? "On" : "Off"}</span>
                {current &&
                  rowActions(target).map(({ label, action, run }) => (
                    <button
                      key={action}
                      className="button"
                      disabled={doing !== undefined}
                      onClick={run}
                    >
                      {label}
                      <Keys action={action} />
                    </button>
                  ))}
              </div>
              <p className="settings-detail code">{mcpSummary(target.transport)}</p>
              {requests.errors[target.id] && (
                <p className="error settings-error">{requests.errors[target.id]}</p>
              )}
            </div>
          );
        })}
      </div>
      {form && machineId && (
        <McpServerSheet
          machineId={machineId}
          server={form.server}
          where={form.server ? `Edit ${form.server.name} on ${deviceName}` : `New MCP server on ${deviceName}`}
          onClose={() => setForm(null)}
        />
      )}
    </div>
  );
}
