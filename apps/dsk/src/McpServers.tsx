import { invoke } from "@tauri-apps/api/core";
import { type KeyboardEvent, useEffect, useState } from "react";
import {
  type Command,
  type CommandOutput,
  type DaemonState,
  type Device,
  errorText,
  type McpProfile,
  type McpServer,
  request,
} from "./engine";
import { act, type ActionId, keysLabel, notify } from "./keybinds";
import { inPane } from "./keys";
import {
  authLabel,
  canEdit,
  canHaveProfiles,
  canLogIn,
  canLogOut,
  canRemove,
  connectorAgent,
  connectorSummary,
  connectors,
  draftOf,
  draftReady,
  entryAuth,
  formatPairs,
  importSummary,
  leftOut,
  libraryServers,
  type McpDraft,
  mcpSummary,
  nameProblem,
  parsePairs,
  profileNameProblem,
  takenNames,
  transportOf,
  usedBy,
  valuesLabel,
} from "./mcp";
import { McpAddSheet } from "./McpAdd";
import { isOnline } from "./model";
import { type PaletteItem, useScreenPaletteItems } from "./Palette";
import { Sheet } from "./Sheets";
import { hint, type Hint, SETTINGS_MOVES, useHints } from "./shortcuts";
import { Icon } from "./ui/Icon";
import { type MenuGroups, MenuButton, RightClickMenu } from "./ui/Menu";
import { GoInHint, listMoves, SettingsPage } from "./ui/SettingsPage";
import { Keys } from "./ui/StatusIcon";

/** Requests about servers and their profiles, each one's progress and last error kept by the
 * ID of what it's about. */
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

  const remove = (server: McpServer) =>
    void run(server.id, "Removing", { command: "remove_mcp_server", id: server.id });

  const removeProfile = (server: McpServer, profile: McpProfile) =>
    void run(profile.id, "Removing", {
      command: "remove_mcp_profile",
      server_id: server.id,
      id: profile.id,
    });

  // The browser comes back to the daemon, and the state says logged in once it has.
  const logIn = async (server: McpServer, profile: McpProfile, busyId = profile.id) => {
    if (!canLogIn(server)) return;
    const output = await run(busyId, "Starting login", {
      command: "login_mcp_server",
      id: server.id,
      profile_id: profile.id,
    });
    if (output?.output !== "mcp_login") return;
    invoke("open_url", { url: output.url }).catch((reason) => fail(busyId, reason));
  };

  const logOut = (server: McpServer, profile: McpProfile, busyId = profile.id) => {
    if (canLogOut(server, profile)) {
      void run(busyId, "Logging out", {
        command: "logout_mcp_server",
        id: server.id,
        profile_id: profile.id,
      });
    }
  };

  return {
    busy,
    errors,
    run,
    remove,
    removeProfile,
    logIn: (server: McpServer, profile: McpProfile, busyId?: string) =>
      void logIn(server, profile, busyId),
    logOut,
  };
}

export type McpRequests = ReturnType<typeof useMcpRequests>;

/** Something a row can do, with the key that does the same. */
export interface RowAction {
  label: string;
  action: ActionId;
  run: () => void;
}

/** Runs whichever of a row's actions this press is the key for. */
export function actOnServer(
  event: globalThis.KeyboardEvent | KeyboardEvent,
  actions: { action: ActionId; run: () => void }[],
): boolean {
  return actions.some(({ action, run }) => act(event, action, run));
}

/** A row's actions as its menu shows them. */
export const actionMenu = (actions: RowAction[]): MenuGroups =>
  actions.length > 0
    ? [actions.map(({ label, action, run }) => ({ id: action, label, keys: keysLabel(action), run }))]
    : [];

const TYPES: McpDraft["type"][] = ["stdio", "http"];

const FORM_HINTS: Hint[] = [
  hint("Save", "mcp_form.save"),
  hint("Stdio or HTTP", "mcp_form.switch_type"),
];

// Enter adds a line in a box of lines, so saving from one takes the save key.
const lines = (event: KeyboardEvent<HTMLTextAreaElement>) => {
  const plain = !event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey;
  if (event.key === "Enter" && plain && !event.nativeEvent.isComposing) event.stopPropagation();
};

/** Changes a server in the library. Adding one goes through `McpAddSheet`, which reads
 * whatever was pasted. */
export function McpServerSheet({
  machineId,
  server,
  where,
  onClose,
}: {
  machineId: string;
  server: McpServer;
  where: string;
  onClose: () => void;
}) {
  const [draft, setDraft] = useState<McpDraft>(() => draftOf(server));
  const change = (next: Partial<McpDraft>) => setDraft((current) => ({ ...current, ...next }));
  const switchType = () => change({ type: draft.type === "stdio" ? "http" : "stdio" });

  const save = async (): Promise<CommandOutput> => {
    const built = transportOf(draft);
    // Shown as is, like the daemon's own messages.
    if ("error" in built) throw built.error;
    return request(machineId, {
      command: "save_mcp_server",
      id: server.id,
      name: draft.name.trim(),
      transport: built.transport,
    });
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
      hint={nameProblem(draft.name.trim()) ?? undefined}
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
        act(event, "mcp_form.switch_type", switchType);
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
      </div>
    </Sheet>
  );
}

const PROFILE_HINTS: Hint[] = [hint("Save", "mcp_form.save")];

/** Whether saving a new profile goes on to log it in. A server that asks for a login gets one
 * right away, because a profile without it is no use to agents. */
const logsInWhenAdded = (server: McpServer, profile: McpProfile) =>
  canLogIn(server) && (profile.auth === "needs_login" || server.needs_login === true);

/** Adds a profile to a server, or changes one when `profile` is given. A new profile of a server
 * that wants a login opens the browser to log in with it. `onSaved` runs before that, so a
 * workspace can start using it. */
export function McpProfileSheet({
  machineId,
  server,
  profile,
  where,
  onSaved,
  onClose,
}: {
  machineId: string;
  server: McpServer;
  profile?: McpProfile;
  where: string;
  onSaved?: (profile: McpProfile) => Promise<unknown>;
  onClose: () => void;
}) {
  const [name, setName] = useState(profile?.name ?? "");
  const [values, setValues] = useState(() => formatPairs(profile?.values));
  const problem = name === "" ? null : profileNameProblem(name, server, profile?.id);

  const save = async (): Promise<CommandOutput> => {
    const parsed = parsePairs(values, valuesLabel(server));
    if ("error" in parsed) throw parsed.error;
    const output = await request(machineId, {
      command: "save_mcp_profile",
      server_id: server.id,
      ...(profile && { id: profile.id }),
      name: name.trim(),
      ...(Object.keys(parsed.pairs).length > 0 && { values: parsed.pairs }),
    });
    if (output.output !== "mcp_profile") return output;
    await onSaved?.(output.profile);
    if (!profile && logsInWhenAdded(server, output.profile)) {
      const login = await request(machineId, {
        command: "login_mcp_server",
        id: server.id,
        profile_id: output.profile.id,
      });
      if (login.output === "mcp_login") await invoke("open_url", { url: login.url });
    }
    return output;
  };

  const loggingIn = !profile && canLogIn(server) && server.needs_login === true;

  return (
    <Sheet
      where={where}
      action={loggingIn ? "Save and log in" : "Save"}
      hint={problem ?? (loggingIn ? "Opens the browser to log in as this profile" : undefined)}
      canSubmit={name.trim() !== "" && problem === null}
      returnsFocus
      run={save}
      onClose={onClose}
      onDone={() => {}}
      hints={PROFILE_HINTS}
      input={`${name}\n${values}`}
      onKeyDown={(event) => {
        const form = event.currentTarget;
        act(event, "mcp_form.save", () => form.requestSubmit());
      }}
    >
      <input
        className="sheet-name"
        autoFocus
        value={name}
        placeholder="Profile name, like work or personal"
        aria-label="Profile name"
        spellCheck={false}
        autoCorrect="off"
        autoCapitalize="off"
        onChange={(event) => setName(event.target.value)}
      />
      <div className="sheet-fields">
        <label className="sheet-field">
          <span className="sheet-label">{valuesLabel(server)}</span>
          <textarea
            className="sheet-input code"
            value={values}
            placeholder="KEY=VALUE, laid over the server's own"
            rows={2}
            spellCheck={false}
            autoCorrect="off"
            autoCapitalize="off"
            onChange={(event) => setValues(event.target.value)}
            onKeyDown={lines}
          />
        </label>
      </div>
    </Sheet>
  );
}

/** What the library is showing a form for. */
type Form =
  | { kind: "add" }
  | { kind: "edit"; server: McpServer }
  | { kind: "profile"; server: McpServer; profile?: McpProfile }
  | null;

/** A row of the library: a server, or one of its profiles under it. */
interface LibraryRow {
  server: McpServer;
  profile?: McpProfile;
}

const rowId = (row: LibraryRow) => row.profile?.id ?? row.server.id;

const usedLabel = (names: string[]) =>
  names.length === 0 ? "In no workspace" : names.join(", ");

/** The device's library of servers, with the profiles workspaces choose between, and its
 * agents' connectors. Which workspace gets which is on each workspace's page. */
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
  const servers = state ? libraryServers(state) : [];
  const connected = state ? connectors(state) : [];
  // One list for the keyboard: each server, then its profiles, then the connectors.
  const serverRows: LibraryRow[] = servers.flatMap((server) => [
    { server },
    ...(server.profiles ?? []).map((profile) => ({ server, profile })),
  ]);
  const rows: LibraryRow[] = [...serverRows, ...connected.map((server) => ({ server }))];
  const requests = useMcpRequests(machineId);
  const [cursor, setCursor] = useState(0);
  const [form, setForm] = useState<Form>(null);
  const [importing, setImporting] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  // What the last import or refresh couldn't do. What it did is a toast.
  const [importError, setImportError] = useState<string | null>(null);

  const selected = Math.max(Math.min(cursor, rows.length - 1), 0);
  const current = rows[selected];
  const deviceName = state?.machine.name ?? device.url;

  const add = () => {
    if (online) setForm({ kind: "add" });
  };

  const runImport = async () => {
    if (!machineId || !online || importing) return;
    setImporting(true);
    setImportError(null);
    try {
      const output = await request(machineId, { command: "import_mcp_servers" });
      if (output.output === "mcp_import") {
        notify(importSummary(output.added, output.skipped));
        setImportError(leftOut(output.skipped, "Skipped"));
      }
    } catch (reason) {
      setImportError(errorText(reason));
    } finally {
      setImporting(false);
    }
  };

  const runRefresh = async () => {
    if (!machineId || !online || refreshing) return;
    setRefreshing(true);
    setImportError(null);
    try {
      const output = await request(machineId, { command: "refresh_mcp_connectors" });
      if (output.output === "mcp_connectors") {
        const summary = connectorSummary(output.added, output.removed, output.problems);
        if (summary) notify(summary);
        setImportError(leftOut(output.problems, "Couldn't list"));
      }
    } catch (reason) {
      setImportError(errorText(reason));
    } finally {
      setRefreshing(false);
    }
  };

  const rowActions = ({ server, profile }: LibraryRow): RowAction[] => {
    if (!online) return [];
    const actions: RowAction[] = [];
    if (profile) {
      actions.push({
        label: "Edit",
        action: "mcp.edit",
        run: () => setForm({ kind: "profile", server, profile }),
      });
      if (canLogIn(server)) {
        actions.push({
          label: profile.auth === "logged_in" ? "Log in again" : "Log in",
          action: "mcp.login",
          run: () => requests.logIn(server, profile),
        });
      }
      if (canLogOut(server, profile)) {
        actions.push({
          label: "Log out",
          action: "mcp.logout",
          run: () => requests.logOut(server, profile),
        });
      }
      actions.push({
        label: "Remove",
        action: "mcp.remove",
        run: () => requests.removeProfile(server, profile),
      });
      return actions;
    }
    if (canEdit(server)) {
      actions.push({ label: "Edit", action: "mcp.edit", run: () => setForm({ kind: "edit", server }) });
    }
    if (canHaveProfiles(server)) {
      actions.push({
        // Logging in makes a profile, so a server that wants one says so here.
        label: server.needs_login && !server.profiles?.length ? "Log in" : "New profile",
        action: "mcp.new_profile",
        run: () => setForm({ kind: "profile", server }),
      });
    }
    if (canRemove(server)) {
      actions.push({ label: "Remove", action: "mcp.remove", run: () => requests.remove(server) });
    }
    return actions;
  };

  useHints(
    "pane",
    active
      ? [
          ...(online
            ? [hint("Add", "mcp.add"), hint("Import", "mcp.import"), hint("Connectors", "mcp.connectors")]
            : []),
          ...(current ? rowActions(current).map(({ label, action }) => hint(label, action)) : []),
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
      {
        id: "mcp-connectors",
        label: `Refresh connectors on ${deviceName}`,
        detail: "The claude.ai connectors and ChatGPT apps of Claude and Codex",
        keys: keysLabel("mcp.connectors"),
        run: then(() => void runRefresh()),
      },
    );
    for (const row of rows) {
      const what = row.profile ? `${row.server.name} profile ${row.profile.name}` : row.server.name;
      for (const { label, action, run } of rowActions(row)) {
        paletteItems.push({
          id: `mcp/${rowId(row)}/${action}`,
          label: `${label}: ${what}`,
          detail: connectorAgent(row.server)
            ? `${connectorAgent(row.server)} connector`
            : "MCP server library",
          keys: row === current && active ? keysLabel(action) : undefined,
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
        ...listMoves(selected, rows.length, setCursor, onLeave),
        ["mcp.add", add],
        ["mcp.import", () => void runImport()],
        ["mcp.connectors", () => void runRefresh()],
      ];
      if (commands.some(([id, run]) => act(event, id, run))) return;
      if (current) actOnServer(event, rowActions(current));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const row = (target: LibraryRow) => {
    const index = rows.indexOf(target);
    const isCurrent = active && index === selected;
    const { server, profile } = target;
    const id = rowId(target);
    const busy = requests.busy[id];
    const login = entryAuth(server, profile);
    const auth = authLabel(login);
    const needsLogin = login === "needs_login";
    const agent = connectorAgent(server);
    const used = usedBy(state ?? EMPTY_STATE, server.id, profile?.id);
    return (
      <div
        key={id}
        className={`keybind ${profile ? "nested" : ""} ${isCurrent ? "current" : ""}`}
        data-row={id}
        // Moving, rather than entering: a row scrolling under a pointer that
        // isn't moving must not take the keyboard or the selection.
        onMouseMove={(event) => {
          if (event.movementX === 0 && event.movementY === 0) return;
          setCursor(index);
          onActivate();
        }}
      >
        <span className={`name ${profile ? "" : "code"}`}>{profile ? profile.name : server.name}</span>
        {!profile && <span className="server-summary">{mcpSummary(server.transport)}</span>}
        {busy ? (
          <span className="meta">{busy}</span>
        ) : requests.errors[id] ? (
          <span className="meta bad">Failed</span>
        ) : auth && (profile || !server.profiles?.length) ? (
          <span className={`meta ${needsLogin ? "alert" : ""}`}>{auth}</span>
        ) : (
          agent && <span className="meta">{agent}</span>
        )}
        <span className="meta" title={used.join(", ")}>
          {usedLabel(used)}
        </span>
        <MenuButton
          label={`Actions for ${profile ? profile.name : server.name}`}
          className="icon-button row-menu"
          groups={actionMenu(rowActions(target))}
        >
          <Icon name="more" />
        </MenuButton>
      </div>
    );
  };

  const doing = current && requests.busy[rowId(current)];
  const rowError = current && requests.errors[rowId(current)];
  const footer = active ? (
    <>
      {rowError ? (
        <span className="settings-actions-what error">{rowError}</span>
      ) : (
        <span className="settings-actions-what code">
          {current ? (doing ?? mcpSummary(current.server.transport)) : ""}
        </span>
      )}
      {current &&
        rowActions(current).map(({ label, action, run }) => (
          <button key={action} className="button" disabled={doing !== undefined} onClick={run}>
            {label}
            <Keys action={action} />
          </button>
        ))}
    </>
  ) : (
    <GoInHint />
  );

  const groupsAt = (target: Element): MenuGroups => {
    const id = target.closest<HTMLElement>("[data-row]")?.dataset.row;
    const found = rows.find((r) => rowId(r) === id);
    return found ? actionMenu(rowActions(found)) : [];
  };

  return (
    <>
      <SettingsPage error={importError} footer={footer}>
        {!online && <p className="settings-note">This device is offline</p>}
        <section className="settings-group">
          {/* What applies to the whole list sits with it. The bar at the bottom is only for
              the selected row. */}
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
          <p className="settings-note">
            Workspaces choose which of these their agents get, and with which profile, on their
            own page.
          </p>
          {online && servers.length === 0 && <p className="settings-note">No servers yet</p>}
          <RightClickMenu render={<div className="keybinds" />} groupsAt={groupsAt}>
            {serverRows.map(row)}
          </RightClickMenu>
        </section>
        <section className="settings-group">
          <div className="settings-group-head">
            <h3 className="settings-group-title">Connectors</h3>
            <button
              className={`button ${connected.length === 0 ? "primary" : ""}`}
              disabled={!online || refreshing}
              onClick={() => void runRefresh()}
            >
              {refreshing ? "Refreshing" : "Refresh"}
              {active && <Keys action="mcp.connectors" />}
            </button>
          </div>
          {online && connected.length === 0 && <p className="settings-note">No connectors yet</p>}
          <RightClickMenu render={<div className="keybinds" />} groupsAt={groupsAt}>
            {rows.slice(serverRows.length).map(row)}
          </RightClickMenu>
        </section>
      </SettingsPage>
      {form?.kind === "edit" && machineId && (
        <McpServerSheet
          machineId={machineId}
          server={form.server}
          where={`Edit ${form.server.name} on ${deviceName}`}
          onClose={() => setForm(null)}
        />
      )}
      {form?.kind === "profile" && machineId && (
        <McpProfileSheet
          machineId={machineId}
          server={form.server}
          profile={form.profile}
          where={
            form.profile
              ? `Edit ${form.server.name} profile ${form.profile.name}`
              : `New ${form.server.name} profile`
          }
          onClose={() => setForm(null)}
        />
      )}
      {form?.kind === "add" && machineId && state && (
        <McpAddSheet
          machineId={machineId}
          taken={takenNames(state.mcp_servers ?? [])}
          where={`New MCP server on ${deviceName}`}
          onAdded={(added) => {
            // A server that wants a login only works once a profile has one.
            const login = added.length === 1 && added[0].needs_login ? added[0] : undefined;
            setForm(login ? { kind: "profile", server: login } : null);
          }}
          onClose={() => setForm((now) => (now?.kind === "add" ? null : now))}
        />
      )}
    </>
  );
}

const EMPTY_STATE: DaemonState = { machine: { id: "", name: "" }, workspaces: [], approvals: [] };
