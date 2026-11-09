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
  takenNames,
  transportOf,
  withEnabled,
} from "./mcp";
import { McpAddSheet } from "./McpAdd";
import { isOnline } from "./model";
import { type PaletteItem, useScreenPaletteItems } from "./Palette";
import { Sheet } from "./Sheets";
import { hint, type Hint, SETTINGS_MOVES, useHints } from "./shortcuts";
import { listMoves, SettingsPage } from "./ui/SettingsPage";
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
          ...SETTINGS_MOVES,
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
    if (active) document.querySelector(".keybind.current")?.scrollIntoView({ block: "nearest" });
  }, [selected, active]);

  useEffect(() => {
    if (!active || form) return;
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (!inPane(event)) return;
      const commands: [ActionId, () => void][] = [
        ...listMoves(selected, servers.length, setCursor, onLeave),
        ["mcp.add", add],
        ["mcp.import", () => void runImport()],
      ];
      if (commands.some(([id, run]) => act(event, id, run))) return;
      if (server) actOnServer(event, rowActions(server));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const doing = server && requests.busy[server.id];
  const serverError = server && requests.errors[server.id];
  const footer = active ? (
    <>
      {serverError ? (
        <span className="settings-actions-what error">{serverError}</span>
      ) : (
        <span className="settings-actions-what code">
          {server ? (doing ?? mcpSummary(server.transport)) : ""}
        </span>
      )}
      {server &&
        rowActions(server).map(({ label, action, run }) => (
          <button key={action} className="button" disabled={doing !== undefined} onClick={run}>
            {label}
            <Keys action={action} />
          </button>
        ))}
    </>
  ) : (
    <span className="settings-actions-what">
      <Keys action="sections.confirm" /> Change these servers
    </span>
  );

  return (
    <>
      <SettingsPage error={importError} footer={footer}>
        {!online && <p className="settings-note">This device is offline</p>}
        {imported?.map((line, index) => (
          <p key={index} className="settings-note">
            {line}
          </p>
        ))}
        <section className="settings-group">
          {/* What applies to the whole list sits with it. The bar at the bottom is only for
              the selected server. */}
          <div className="settings-group-head">
            <h3 className="settings-group-title">Servers</h3>
            {/* With nothing set up yet, bringing over what the agents already had is the first step. */}
            <button
              className={`button ${servers.length === 0 ? "primary" : ""}`}
              disabled={!online || importing}
              onClick={() => void runImport()}
            >
              {importing ? "Importing" : "Import"}
              {active && <Keys action="mcp.import" />}
            </button>
            <button
              className={`button ${servers.length === 0 ? "" : "primary"}`}
              disabled={!online}
              onClick={add}
            >
              Add
              {active && <Keys action="mcp.add" />}
            </button>
          </div>
          {online && servers.length === 0 && (
            <p className="settings-note">
              No servers yet, so agents get none. Import brings over the ones Claude Code, Codex
              and OpenCode have. Add takes a URL, a command or a config from the docs.
            </p>
          )}
          <div className="keybinds">
            {servers.map((target, index) => {
              const current = active && index === selected;
              const busy = requests.busy[target.id];
              const auth = mcpAuthLabel(target);
              return (
                <div
                  key={target.id}
                  className={`keybind ${current ? "current" : ""}`}
                  // Moving, rather than entering: a row scrolling under a pointer that
                  // isn't moving must not take the keyboard or the selection.
                  onMouseMove={(event) => {
                    if (event.movementX === 0 && event.movementY === 0) return;
                    setCursor(index);
                    onActivate();
                  }}
                >
                  <span className="name code">{target.name}</span>
                  {busy ? (
                    <span className="meta">{busy}</span>
                  ) : requests.errors[target.id] ? (
                    <span className="meta bad">Failed</span>
                  ) : (
                    auth && (
                      <span className={`meta ${target.auth === "needs_login" ? "alert" : ""}`}>
                        {auth}
                      </span>
                    )
                  )}
                  <button
                    className="picker"
                    tabIndex={-1}
                    role="switch"
                    aria-checked={target.enabled}
                    aria-label={`${target.name} on for topics by default`}
                    disabled={!online || !canEdit(target)}
                    onMouseDown={(event) => event.preventDefault()}
                    onClick={() => requests.toggle(target)}
                  >
                    {target.enabled ? "On" : "Off"}
                    {current && online && canEdit(target) && <Keys action="mcp.toggle" />}
                  </button>
                </div>
              );
            })}
          </div>
        </section>
        <p className="settings-note settings-group">
          Topics get the servers that are on, and each topic can change that.
        </p>
      </SettingsPage>
      {form?.server && machineId && (
        <McpServerSheet
          machineId={machineId}
          server={form.server}
          where={`Edit ${form.server.name} on ${deviceName}`}
          onClose={() => setForm(null)}
        />
      )}
      {form && !form.server && machineId && state && (
        <McpAddSheet
          machineId={machineId}
          taken={takenNames(state.mcp_servers ?? [])}
          where={`New MCP server on ${deviceName}`}
          onClose={() => setForm(null)}
        />
      )}
    </>
  );
}
