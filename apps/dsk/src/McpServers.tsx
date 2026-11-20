import { invoke } from "@tauri-apps/api/core";
import { type KeyboardEvent, useState } from "react";
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
import { act, notify } from "./keybinds";
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
import { Sheet } from "./Sheets";
import { Icon } from "./ui/Icon";
import { type MenuGroups, MenuButton, RightClickMenu } from "./ui/Menu";
import { SettingRow, SettingsGroupBox, SettingsPage } from "./ui/SettingsPage";

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

/** Something a row can do. These are done now and then, so they're in the row's menu rather
 * than keys. */
export interface RowAction {
  id: string;
  label: string;
  run: () => void;
}

/** A row's actions as its menu shows them. */
export const actionMenu = (actions: RowAction[]): MenuGroups =>
  actions.length > 0 ? [actions.map(({ id, label, run }) => ({ id, label, run }))] : [];

const TYPES: McpDraft["type"][] = ["stdio", "http"];

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
      input={JSON.stringify(draft)}
      onKeyDown={(event) => {
        const form = event.currentTarget;
        act(event, "mcp_form.save", () => form.requestSubmit());
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
                // A click leaves the keyboard in the field being typed in. Tab still reaches it.
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => change({ type })}
              >
                {type === "stdio" ? "Stdio" : "HTTP"}
              </button>
            ))}
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
export function McpSettings({ device }: { device: Device }) {
  const state = device.state;
  const machineId = state?.machine.id;
  const online = isOnline(device);
  const servers = state ? libraryServers(state) : [];
  const connected = state ? connectors(state) : [];
  // Each server, then its profiles under it.
  const serverRows: LibraryRow[] = servers.flatMap((server) => [
    { server },
    ...(server.profiles ?? []).map((profile) => ({ server, profile })),
  ]);
  const connectorRows: LibraryRow[] = connected.map((server) => ({ server }));
  const rows: LibraryRow[] = [...serverRows, ...connectorRows];
  const requests = useMcpRequests(machineId);
  const [form, setForm] = useState<Form>(null);
  const [importing, setImporting] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  // What the last import or refresh couldn't do. What it did is a toast.
  const [importError, setImportError] = useState<string | null>(null);
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
        id: "edit",
        run: () => setForm({ kind: "profile", server, profile }),
      });
      if (canLogIn(server)) {
        actions.push({
          label: profile.auth === "logged_in" ? "Log in again" : "Log in",
          id: "login",
          run: () => requests.logIn(server, profile),
        });
      }
      if (canLogOut(server, profile)) {
        actions.push({
          label: "Log out",
          id: "logout",
          run: () => requests.logOut(server, profile),
        });
      }
      actions.push({
        label: "Remove",
        id: "remove",
        run: () => requests.removeProfile(server, profile),
      });
      return actions;
    }
    if (canEdit(server)) {
      actions.push({ id: "edit", label: "Edit", run: () => setForm({ kind: "edit", server }) });
    }
    if (canHaveProfiles(server)) {
      actions.push({
        // Logging in makes a profile, so a server that wants one says so here.
        label: server.needs_login && !server.profiles?.length ? "Log in" : "New profile",
        id: "new-profile",
        run: () => setForm({ kind: "profile", server }),
      });
    }
    if (canRemove(server)) {
      actions.push({ id: "remove", label: "Remove", run: () => requests.remove(server) });
    }
    return actions;
  };

  /** Logging in is the one thing a row that needs it is waiting for, so it's on the row itself. */
  const logIn = ({ server, profile }: LibraryRow) => {
    if (profile) requests.logIn(server, profile);
    else setForm({ kind: "profile", server });
  };

  const row = (target: LibraryRow) => {
    const { server, profile } = target;
    const id = rowId(target);
    const busy = requests.busy[id];
    const login = entryAuth(server, profile);
    const auth = authLabel(login);
    const needsLogin = login === "needs_login";
    const agent = connectorAgent(server);
    const used = usedBy(state ?? EMPTY_STATE, server.id, profile?.id);
    const showsAuth = auth && (profile || !server.profiles?.length);
    const name = profile ? profile.name : server.name;
    return (
      <div key={id} data-row={id}>
        <SettingRow
          label={profile ? name : <span className="code">{name}</span>}
          detail={profile ? undefined : mcpSummary(server.transport)}
          nested={profile !== undefined}
          error={requests.errors[id]}
          meta={
            <>
              {busy ? (
                <span className="meta">{busy}</span>
              ) : showsAuth && !(needsLogin && online && canLogIn(server)) ? (
                <span className={`meta ${needsLogin ? "alert" : ""}`}>{auth}</span>
              ) : (
                agent && <span className="meta">{agent}</span>
              )}
              <span className="meta" title={used.join(", ")}>
                {usedLabel(used)}
              </span>
            </>
          }
          quiet={
            <MenuButton
              label={`Actions for ${name}`}
              className="icon-button"
              groups={actionMenu(rowActions(target))}
            >
              <Icon name="more" />
            </MenuButton>
          }
        >
          {!busy && showsAuth && needsLogin && online && canLogIn(server) && (
            <button className="picker alert" onClick={() => logIn(target)}>
              Log in
            </button>
          )}
        </SettingRow>
      </div>
    );
  };

  const groupsAt = (target: Element): MenuGroups => {
    const id = target.closest<HTMLElement>("[data-row]")?.dataset.row;
    const found = rows.find((r) => rowId(r) === id);
    return found ? actionMenu(rowActions(found)) : [];
  };

  return (
    <>
      <SettingsPage error={importError}>
        {!online && <p className="settings-note">This device is offline</p>}
        <SettingsGroupBox
          title="Servers"
          actions={
            <>
              {/* With nothing set up yet, bringing over what the agents already had is the first step. */}
              <button
                className={`button ${servers.length === 0 ? "primary" : ""}`}
                disabled={!online || importing}
                onClick={() => void runImport()}
              >
                {importing ? "Importing" : "Import"}
              </button>
              <button
                className={`button ${servers.length === 0 ? "" : "primary"}`}
                disabled={!online}
                onClick={add}
              >
                Add
              </button>
            </>
          }
        >
          <p className="settings-note">
            Workspaces choose which of these their agents get, and with which profile, on their
            own page.
          </p>
          {online && servers.length === 0 && <p className="settings-note">No servers yet</p>}
          <RightClickMenu render={<div className="settings-rows" />} groupsAt={groupsAt}>
            {serverRows.map(row)}
          </RightClickMenu>
        </SettingsGroupBox>
        <SettingsGroupBox
          title="Connectors"
          actions={
            <button
              className={`button ${connected.length === 0 ? "primary" : ""}`}
              disabled={!online || refreshing}
              onClick={() => void runRefresh()}
            >
              {refreshing ? "Refreshing" : "Refresh"}
            </button>
          }
        >
          {online && connected.length === 0 && <p className="settings-note">No connectors yet</p>}
          <RightClickMenu render={<div className="settings-rows" />} groupsAt={groupsAt}>
            {connectorRows.map(row)}
          </RightClickMenu>
        </SettingsGroupBox>
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
