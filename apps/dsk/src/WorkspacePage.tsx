import { type Ref, useEffect, useRef, useState } from "react";
import {
  type ArchivedTopic,
  type Device,
  errorText,
  type McpServer,
  request,
  type Topic,
  type Workspace,
  type WorkspaceMcp,
} from "./engine";
import {
  addableServers,
  authLabel,
  canEdit,
  canHaveProfiles,
  canLogIn,
  canLogOut,
  connectorAgent,
  copiedMcp,
  entryAuth,
  mcpSummary,
  NO_PROFILE,
  newEntry,
  takenNames,
  withServer,
  type WorkspaceServer,
  workspaceServers,
} from "./mcp";
import { McpAddSheet } from "./McpAdd";
import {
  actionMenu,
  McpProfileSheet,
  McpServerSheet,
  type RowAction,
  useMcpRequests,
} from "./McpServers";
import { isOnline, STATE_LABEL, tabsOf, topicState } from "./model";
import { Icon } from "./ui/Icon";
import { MenuButton, type MenuGroups, RightClickMenu } from "./ui/Menu";
import { Choice, Picker } from "./ui/Picker";
import { onRowKeys, SettingRow, Switch, useRovingList } from "./ui/SettingsPage";
import { StatusIcon } from "./ui/StatusIcon";

type PageRow =
  | { kind: "topic"; topic: Topic }
  /** What an empty section offers instead of rows. */
  | { kind: "action"; id: string; label: string; run: () => void };

type Form =
  | { kind: "add" }
  | { kind: "edit"; server: McpServer }
  | { kind: "profile"; server: McpServer }
  | null;

/** What busy and error states about the whole list are kept under. */
const WHOLE_LIST = "workspace";

const NEW_PROFILE = "new";
const NONE = "none";

const counted = (count: number, thing: string) => `${count} ${thing}${count === 1 ? "" : "s"}`;

const archivedOn = (ms: number) =>
  new Date(ms).toLocaleDateString(undefined, { month: "short", day: "numeric" });

/** What a library server is, in the line under its name. */
const serverDetail = (server: McpServer) => {
  const agent = connectorAgent(server);
  if (agent) return `${agent} connector`;
  return [mcpSummary(server.transport), server.profiles?.map((p) => p.name).join(", ")]
    .filter(Boolean)
    .join(", ");
};

/** The home folder as `~`, which is how people read their own paths. */
const homeShort = (path: string) => path.replace(/^\/(Users|home)\/[^/]+(?=\/|$)/, "~");

/** A workspace's own page: its topics, and the MCP servers its agents get. The topics are one
 * Tab stop with the arrows moving through them. Every server's switch, profile and menu is a
 * control used where it sits, in the Tab order. What's archived and what the library has to add
 * open in place under their lists, so nothing floats over the page. */
export function WorkspacePage({
  device,
  workspace,
  onOpenTopic,
  onNewTopic,
}: {
  device: Device;
  workspace: Workspace;
  onOpenTopic: (topicId: string) => void;
  onNewTopic: () => void;
}) {
  const state = device.state;
  const machineId = state?.machine.id;
  const online = isOnline(device);
  const deviceName = state?.machine.name ?? device.url;
  const servers = state ? workspaceServers(state, workspace) : [];
  const requests = useMcpRequests(machineId);
  const [form, setForm] = useState<Form>(null);
  // The server whose profile dropdown is open.
  const [profileFor, setProfileFor] = useState<string | null>(null);
  // A server just added, which is scrolled into view once it's in the list.
  const [landOn, setLandOn] = useState<string | null>(null);
  const page = useRef<HTMLDivElement>(null);
  const [archivedOpen, setArchivedOpen] = useState(false);
  // Null while they're being asked for.
  const [archived, setArchived] = useState<ArchivedTopic[] | null>(null);
  const [archivedError, setArchivedError] = useState<string | null>(null);
  // Whether the library is open under the servers. Until someone says, it's open while the
  // workspace has none, which is when adding is what the section is for.
  const [libraryChoice, setLibraryChoice] = useState<boolean | null>(null);
  const libraryOpen = libraryChoice ?? servers.length === 0;
  const [copyOpen, setCopyOpen] = useState(false);
  // Opening the library from a button elsewhere takes the keyboard there once it has rendered.
  const focusLibrary = useRef(false);
  const libraryArea = useRef<HTMLDivElement>(null);
  const archivedToggle = useRef<HTMLButtonElement>(null);

  const library = state?.mcp_servers ?? [];
  /** Each other workspace that would add something, with what copying it would make. */
  const copySources = (state?.workspaces ?? []).flatMap((other) => {
    if (other.id === workspace.id) return [];
    const next = copiedMcp(workspace.mcp, other.mcp, library);
    const gained = library.filter((s) => next[s.id] && !workspace.mcp?.[s.id]);
    return gained.length > 0 ? [{ other, next, gained }] : [];
  });

  const emptyTopics: PageRow[] = [
    { kind: "action", id: "new-topic", label: "New topic", run: () => onNewTopic() },
  ];
  const topicRows: PageRow[] =
    workspace.topics.length > 0 || !online
      ? workspace.topics.map((topic): PageRow => ({ kind: "topic", topic }))
      : emptyTopics;
  const addable = state ? addableServers(state, workspace) : [];
  const topics = useRovingList(topicRows.length);

  // Asked for each time the list opens, because archiving happens anywhere.
  useEffect(() => {
    if (!archivedOpen || !machineId || !online) return;
    let shown = true;
    setArchived(null);
    setArchivedError(null);
    request(machineId, { command: "list_archived_topics" })
      .then((output) => {
        if (!shown) return;
        const topics = output.output === "archived_topics" ? output.topics : [];
        setArchived(
          topics
            .filter((topic) => topic.workspace_id === workspace.id)
            .sort((a, b) => b.archived_at_ms - a.archived_at_ms),
        );
      })
      .catch((reason) => {
        if (shown) setArchivedError(errorText(reason));
      });
    return () => {
      shown = false;
    };
  }, [archivedOpen, machineId, online, workspace.id]);

  useEffect(() => {
    if (!focusLibrary.current) return;
    focusLibrary.current = false;
    libraryArea.current?.querySelector<HTMLElement>("[data-setting]")?.focus();
  });

  useEffect(() => {
    if (!landOn) return;
    const row = page.current?.querySelector(`[data-server="${landOn}"]`);
    if (!row) return;
    row.scrollIntoView({ block: "nearest" });
    setLandOn(null);
  });

  const setMcp = (next: Record<string, WorkspaceMcp>, busyId: string, doing: string) =>
    requests.run(busyId, doing, {
      command: "set_workspace_mcp",
      workspace_id: workspace.id,
      servers: next,
    });

  const setEntry = (server: McpServer, entry: WorkspaceMcp | null, doing = "Saving") =>
    void setMcp(withServer(workspace.mcp, server.id, entry), server.id, doing);

  /** Adds a server and lands on it. One with profiles to choose between opens its choice, and
   * one that wants a login it has no profile for goes straight to making one. */
  const addServer = async (server: McpServer) => {
    const entry = newEntry(server);
    setLandOn(server.id);
    const output = await setMcp(withServer(workspace.mcp, server.id, entry), server.id, "Adding");
    if (!output) return;
    if (!entry.profile_id && (server.profiles?.length ?? 0) > 1) setProfileFor(server.id);
    else if (canLogIn(server) && server.needs_login && !server.profiles?.length) {
      setForm({ kind: "profile", server });
    }
  };

  /** Opens the library under the servers and gives it the keyboard, for the section's + and the
   * page menu. */
  const openLibrary = () => {
    if (!online) return;
    focusLibrary.current = true;
    setLibraryChoice(true);
  };

  const copyFrom = (workspaceId: string) => {
    const source = copySources.find(({ other }) => other.id === workspaceId);
    if (source && online) void setMcp(source.next, WHOLE_LIST, "Copying");
  };

  const restore = async (topic: ArchivedTopic) => {
    const output = await requests.run(`archived/${topic.id}`, "Restoring", {
      command: "restore_topic",
      topic_id: topic.id,
    });
    if (!output) return;
    setArchived((now) => now?.filter((other) => other.id !== topic.id) ?? null);
    onOpenTopic(topic.id);
  };

  const serverActions = ({ server, entry, profile }: WorkspaceServer): RowAction[] => {
    if (!online) return [];
    const actions: RowAction[] = [
      {
        id: "toggle",
        label: entry.enabled ? "Turn off" : "Turn on",
        run: () => setEntry(server, { ...entry, enabled: !entry.enabled }),
      },
    ];
    if (canHaveProfiles(server)) {
      actions.push(
        { id: "profile", label: "Profile", run: () => setProfileFor(server.id) },
        { id: "new-profile", label: "New profile", run: () => setForm({ kind: "profile", server }) },
      );
    }
    if (canLogIn(server) && (profile || server.needs_login)) {
      actions.push({
        id: "login",
        label: profile?.auth === "logged_in" ? "Log in again" : "Log in",
        // A login belongs to a profile, so without one, logging in makes one.
        run: () =>
          profile ? requests.logIn(server, profile, server.id) : setForm({ kind: "profile", server }),
      });
    }
    if (profile && canLogOut(server, profile)) {
      actions.push({
        id: "logout",
        label: "Log out",
        run: () => requests.logOut(server, profile, server.id),
      });
    }
    if (canEdit(server)) {
      actions.push({ id: "edit", label: "Edit", run: () => setForm({ kind: "edit", server }) });
    }
    actions.push({
      id: "remove",
      label: "Remove from workspace",
      run: () => setEntry(server, null, "Removing"),
    });
    return actions;
  };

  const canCopy = online && copySources.length > 0;

  const topicRow = (topic: Topic, index: number) => {
    const status = topicState(topic, online);
    const tabs = tabsOf(topic).length;
    // Idle is what the icon already says, so only what needs a look gets words.
    const loud = status === "waiting" || status === "failed";
    return (
      <button
        key={topic.id}
        className="setting-line workspace-topic"
        {...topics.item(index)}
        data-setting=""
        onClick={() => onOpenTopic(topic.id)}
      >
        <StatusIcon state={status} />
        <span className="name">{topic.name}</span>
        {loud && (
          <span className={`meta ${status === "waiting" ? "alert" : "bad"}`}>{STATE_LABEL[status]}</span>
        )}
        <span className="meta">{counted(tabs, "tab")}</span>
      </button>
    );
  };

  /** A row that makes something, like the empty section's New topic: an add icon and what it
   * makes, an ordinary button in the Tab order. */
  const addRow = (id: string, label: string, run: () => void) => (
    <button
      key={id}
      className="setting-line workspace-empty"
      data-setting=""
      disabled={!online}
      onClick={run}
    >
      <Icon name="add" />
      <span className="name">{label}</span>
    </button>
  );

  /** Opens and closes what's under a list, saying how much is there. */
  const disclosure = (
    label: string,
    open: boolean,
    onToggle: () => void,
    count?: number,
    ref?: Ref<HTMLButtonElement>,
  ) => (
    <button
      ref={ref}
      className="workspace-disclosure"
      aria-expanded={open}
      data-setting=""
      disabled={!online}
      onClick={onToggle}
    >
      <Icon name={open ? "fold" : "unfold"} />
      <span>{label}</span>
      {count !== undefined && count > 0 && <span className="workspace-count">{count}</span>}
    </button>
  );

  const archivedRows = () => {
    if (archivedError) return <p className="error workspace-error">{archivedError}</p>;
    if (archived === null) return <p className="settings-note workspace-note">Loading</p>;
    if (archived.length === 0) return <p className="settings-note workspace-note">No archived topics</p>;
    return (
      <div className="settings-rows">
        {archived.map((topic) => {
          const id = `archived/${topic.id}`;
          const busy = requests.busy[id];
          return (
            <SettingRow
              key={topic.id}
              label={topic.name}
              dim
              meta={<span className="meta">{busy ?? `Archived ${archivedOn(topic.archived_at_ms)}`}</span>}
              error={requests.errors[id]}
            >
              <button
                className="button small"
                disabled={!online || busy !== undefined}
                onClick={() => void restore(topic)}
              >
                Restore
              </button>
            </SettingRow>
          );
        })}
      </div>
    );
  };

  /** What the device's library has that this workspace doesn't, each a row away from added,
   * then copying another workspace's, then making a new one. */
  const libraryRows = () => (
    <div ref={libraryArea} className="settings-rows workspace-library">
      {addable.length === 0 && (
        <p className="settings-note workspace-note">Every server in the library is here</p>
      )}
      {addable.map((server) => {
        const busy = requests.busy[server.id];
        return (
          <SettingRow
            key={server.id}
            label={server.name}
            detail={serverDetail(server)}
            meta={busy ? <span className="meta">{busy}</span> : undefined}
            error={requests.errors[server.id]}
          >
            <button
              className="button small"
              disabled={!online || busy !== undefined}
              onClick={() => void addServer(server)}
            >
              Add
            </button>
          </SettingRow>
        );
      })}
      {copySources.length > 0 && (
        <SettingRow
          label="Copy from another workspace"
          meta={requests.busy[WHOLE_LIST] ? <span className="meta">{requests.busy[WHOLE_LIST]}</span> : undefined}
        >
          <Picker
            label={`Copy MCP servers into ${workspace.name} from`}
            className="picker"
            value={null}
            disabled={!online}
            open={copyOpen}
            onOpenChange={setCopyOpen}
            onChange={copyFrom}
            trigger={
              <>
                <span className="dim">Workspace</span>
                <Icon name="fold" />
              </>
            }
          >
            {copySources.map(({ other, gained }) => (
              <Choice
                key={other.id}
                value={other.id}
                label={other.name}
                meta={`Adds ${gained.map((s) => s.name).join(", ")}`}
              />
            ))}
          </Picker>
        </SettingRow>
      )}
      {addRow("new-server", "New server...", () => setForm({ kind: "add" }))}
    </div>
  );

  const serverRow = (item: WorkspaceServer) => {
    const { server, entry, profile } = item;
    const busy = requests.busy[server.id];
    const auth = entryAuth(server, profile);
    const agent = connectorAgent(server);
    const hasProfiles = (server.profiles?.length ?? 0) > 0;
    const failed = requests.errors[server.id];
    return (
      <div key={server.id} data-server={server.id}>
        <SettingRow
          label={server.name}
          detail={agent ? `${agent} connector` : mcpSummary(server.transport)}
          dim={!entry.enabled}
          error={failed}
          // Left turns it off and right on, the way its switch slides.
          onStep={(direction) => {
            const enabled = direction > 0;
            if (online && enabled !== entry.enabled) setEntry(server, { ...entry, enabled });
          }}
          meta={busy ? <span className="meta">{busy}</span> : undefined}
          quiet={
            <MenuButton
              label={`Actions for ${server.name}`}
              className="icon-button"
              groups={actionMenu(serverActions(item))}
            >
              <Icon name="more" />
            </MenuButton>
          }
        >
          {!busy && auth === "needs_login" && online && (
            <button
              className="picker alert"
              onClick={() =>
                profile ? requests.logIn(server, profile, server.id) : setForm({ kind: "profile", server })
              }
            >
              Log in
            </button>
          )}
          {canHaveProfiles(server) && (
            <Picker
              label={`Profile ${server.name} uses`}
              className="picker"
              value={profile?.id ?? NONE}
              disabled={!online}
              open={profileFor === server.id}
              onOpenChange={(open) => setProfileFor(open ? server.id : null)}
              onChange={(value) => {
                setProfileFor(null);
                if (value === NEW_PROFILE) setForm({ kind: "profile", server });
                else setEntry(server, { ...entry, profile_id: value === NONE ? undefined : value });
              }}
              trigger={
                <>
                  <span className={profile ? "" : "dim"}>
                    {profile ? profile.name : hasProfiles ? NO_PROFILE : "Profile"}
                  </span>
                  <Icon name="fold" />
                </>
              }
            >
              {(server.profiles ?? []).map((p) => (
                <Choice
                  key={p.id}
                  value={p.id}
                  label={p.name}
                  meta={authLabel(p.auth === "logged_in" ? "logged_in" : null) ?? undefined}
                />
              ))}
              <Choice value={NONE} label={NO_PROFILE} />
              <Choice value={NEW_PROFILE} label="New profile..." />
            </Picker>
          )}
          <Switch
            label={`${server.name} on in ${workspace.name}`}
            title={entry.enabled ? "Turn off" : "Turn on"}
            checked={entry.enabled}
            disabled={!online}
            onChange={(enabled) => setEntry(server, { ...entry, enabled })}
          />
        </SettingRow>
      </div>
    );
  };

  const serverMenuAt = (target: Element): MenuGroups => {
    const id = target.closest<HTMLElement>("[data-server]")?.dataset.server;
    const item = servers.find((s) => s.server.id === id);
    return item ? actionMenu(serverActions(item)) : [];
  };

  const openArchived = () => {
    setArchivedOpen(true);
    archivedToggle.current?.focus();
  };

  const pageMenu: MenuGroups = online
    ? [
        [
          { id: "new-topic", label: "New topic", run: onNewTopic },
          { id: "archived-topics", label: "Show archived topics", run: openArchived },
        ],
        [
          { id: "add-server", label: "Add MCP server", run: openLibrary },
          // Few enough to name them all, so the menu holds them rather than a list of its own.
          ...(canCopy
            ? [
                {
                  id: "copy-servers",
                  label: "Copy MCP servers from",
                  run: () => {},
                  items: copySources.map(({ other }) => ({
                    id: `copy/${other.id}`,
                    label: other.name,
                    run: () => copyFrom(other.id),
                  })),
                },
              ]
            : []),
        ],
      ]
    : [];

  const section = (title: string, count: number, add: { label: string; run: () => void }) => (
    <div className="workspace-section-head">
      <h2 className="workspace-section-title">{title}</h2>
      {count > 0 && <span className="workspace-count">{count}</span>}
      <button
        className="icon-button"
        disabled={!online}
        title={add.label}
        aria-label={add.label}
        onClick={add.run}
      >
        <Icon name="add" />
      </button>
    </div>
  );

  return (
    <>
      <div
        ref={page}
        className="pane settings workspace-page"
        // Where the keyboard lands when the page opens, so Tab and the row keys start from the
        // top. Not a Tab stop. Esc goes back to the sidebar, which App does for every pane.
        tabIndex={-1}
        onKeyDown={(event) => onRowKeys(event)}
      >
        <div className="settings-in workspace-in">
          <header className="workspace-head">
            <div className="workspace-heading">
              <h1 className="workspace-title">{workspace.name}</h1>
              <p className="workspace-sub">
                <span>{online ? deviceName : `${deviceName} is offline`}</span>
                <span className="code" title={workspace.path}>
                  {homeShort(workspace.path)}
                </span>
              </p>
            </div>
            {pageMenu.length > 0 && (
              <MenuButton label={`Actions for ${workspace.name}`} className="icon-button" groups={pageMenu}>
                <Icon name="more" />
              </MenuButton>
            )}
          </header>
          <section className="workspace-section">
            {section("Topics", workspace.topics.length, {
              label: "New topic",
              run: onNewTopic,
            })}
            {/* One Tab stop, with the row keys moving through the topics and on to the servers. */}
            <div className="settings-rows">
              {topicRows.map((row, index) =>
                row.kind === "topic" ? topicRow(row.topic, index) : addRow(row.id, row.label, row.run),
              )}
            </div>
            {disclosure(
              "Archived topics",
              archivedOpen,
              () => setArchivedOpen((open) => !open),
              undefined,
              archivedToggle,
            )}
            {archivedOpen && archivedRows()}
          </section>
          <section className="workspace-section">
            {section("MCP servers", servers.length, {
              label: "Add MCP server",
              run: openLibrary,
            })}
            {servers.length > 0 && (
              <RightClickMenu render={<div className="settings-rows" />} groupsAt={serverMenuAt}>
                {servers.map(serverRow)}
              </RightClickMenu>
            )}
            {online && state && (
              <>
                {disclosure("Add from the library", libraryOpen, () => setLibraryChoice(!libraryOpen), addable.length)}
                {libraryOpen && libraryRows()}
              </>
            )}
            {requests.errors[WHOLE_LIST] && <p className="error workspace-error">{requests.errors[WHOLE_LIST]}</p>}
          </section>
        </div>
      </div>
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
          where={`New ${form.server.name} profile for ${workspace.name}`}
          onSaved={(profile) => {
            const entry = workspace.mcp?.[form.server.id] ?? newEntry(form.server);
            return request(machineId, {
              command: "set_workspace_mcp",
              workspace_id: workspace.id,
              servers: withServer(workspace.mcp, form.server.id, { ...entry, profile_id: profile.id }),
            });
          }}
          onClose={() => setForm(null)}
        />
      )}
      {form?.kind === "add" && machineId && state && (
        <McpAddSheet
          machineId={machineId}
          taken={takenNames(state.mcp_servers ?? [])}
          where={`New MCP server for ${workspace.name}`}
          onAdded={(added) => {
            let next = workspace.mcp;
            for (const server of added) next = withServer(next, server.id, newEntry(server));
            void setMcp(next ?? {}, WHOLE_LIST, "Adding");
            const first = added[0];
            if (first) setLandOn(first.id);
            // A server that wants a login only works here once a profile has one.
            const login = added.length === 1 && first?.needs_login ? first : undefined;
            setForm(login ? { kind: "profile", server: login } : null);
          }}
          onClose={() => setForm((now) => (now?.kind === "add" ? null : now))}
        />
      )}
    </>
  );
}
