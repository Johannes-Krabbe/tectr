import { type MouseEvent, useEffect, useRef, useState } from "react";
import { type Device, type McpServer, request, type Topic, type Workspace, type WorkspaceMcp } from "./engine";
import { act, type ActionId, keysLabel } from "./keybinds";
import { inPane } from "./keys";
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
  actOnServer,
  McpProfileSheet,
  McpServerSheet,
  type RowAction,
  useMcpRequests,
} from "./McpServers";
import { isOnline, STATE_LABEL, tabsOf, topicState } from "./model";
import { type PaletteItem, useScreenPaletteItems } from "./Palette";
import { hint, useHints } from "./shortcuts";
import { Icon } from "./ui/Icon";
import { MenuButton, type MenuGroups, RightClickMenu } from "./ui/Menu";
import { Choice, Picker } from "./ui/Picker";
import { StatusIcon } from "./ui/StatusIcon";

/** A list to choose from in the palette's place. */
export interface PagePicker {
  placeholder: string;
  empty: string;
  items: PaletteItem[];
}

type PageRow =
  | { kind: "topic"; topic: Topic }
  | { kind: "server"; item: WorkspaceServer }
  /** What an empty section offers instead of rows, reachable like a row. */
  | { kind: "action"; id: string; icon: "add" | "copy"; label: string; run: () => void };

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

/** The home folder as `~`, which is how people read their own paths. */
const homeShort = (path: string) => path.replace(/^\/(Users|home)\/[^/]+(?=\/|$)/, "~");

/** A tooltip that teaches the key without putting it on screen. */
const withKey = (label: string, action: ActionId) => {
  const keys = keysLabel(action);
  return keys ? `${label}  ${keys}` : label;
};

/** A workspace's own page: its topics, and the MCP servers its agents get. Keys stay out of
 * sight: every action is a quiet button or in a menu, and tooltips and menus name the keys. */
export function WorkspacePage({
  device,
  workspace,
  onOpenTopic,
  onNewTopic,
  onPicker,
}: {
  device: Device;
  workspace: Workspace;
  onOpenTopic: (topicId: string) => void;
  onNewTopic: () => void;
  onPicker: (picker: PagePicker) => void;
}) {
  const state = device.state;
  const machineId = state?.machine.id;
  const online = isOnline(device);
  const deviceName = state?.machine.name ?? device.url;
  const servers = state ? workspaceServers(state, workspace) : [];
  const requests = useMcpRequests(machineId);
  const [cursor, setCursor] = useState(0);
  const [form, setForm] = useState<Form>(null);
  // The server whose profile dropdown is open.
  const [profileFor, setProfileFor] = useState<string | null>(null);
  // A server just added, which the cursor goes to once it's in the list.
  const [landOn, setLandOn] = useState<string | null>(null);
  const page = useRef<HTMLDivElement>(null);

  const library = state?.mcp_servers ?? [];
  /** Each other workspace that would add something, with what copying it would make. */
  const copySources = (state?.workspaces ?? []).flatMap((other) => {
    if (other.id === workspace.id) return [];
    const next = copiedMcp(workspace.mcp, other.mcp, library);
    const gained = library.filter((s) => next[s.id] && !workspace.mcp?.[s.id]);
    return gained.length > 0 ? [{ other, next, gained }] : [];
  });

  const copyFrom = () => {
    if (!online) return;
    onPicker({
      placeholder: `Copy MCP servers into ${workspace.name} from`,
      empty: `No other workspace on ${deviceName} has servers this one doesn't`,
      items: copySources.map(({ other, next, gained }) => ({
        id: `copy/${other.id}`,
        label: other.name,
        detail: `Adds ${gained.map((s) => s.name).join(", ")}`,
        run: () => void setMcp(next, WHOLE_LIST, "Copying"),
      })),
    });
  };

  const emptyTopics: PageRow[] = [
    { kind: "action", id: "new-topic", icon: "add", label: "New topic", run: () => onNewTopic() },
  ];
  const emptyServers: PageRow[] = [
    {
      kind: "action",
      id: "add-server",
      icon: "add",
      label: "Add an MCP server",
      run: () => addFromLibrary(),
    },
  ];
  // Copying only shows up where it helps: in an empty list, with somewhere to copy from.
  if (copySources.length > 0) {
    emptyServers.push({
      kind: "action",
      id: "copy-servers",
      icon: "copy",
      label: "Copy from another workspace",
      run: () => copyFrom(),
    });
  }
  const topicRows: PageRow[] =
    workspace.topics.length > 0 || !online
      ? workspace.topics.map((topic): PageRow => ({ kind: "topic", topic }))
      : emptyTopics;
  const serverRows: PageRow[] =
    servers.length > 0 || !online
      ? servers.map((item): PageRow => ({ kind: "server", item }))
      : emptyServers;
  const rows = [...topicRows, ...serverRows];

  const selected = Math.max(Math.min(cursor, rows.length - 1), 0);
  const current = rows[selected];

  useEffect(() => {
    if (!landOn) return;
    const index = rows.findIndex((row) => row.kind === "server" && row.item.server.id === landOn);
    if (index === -1) return;
    setCursor(index);
    setLandOn(null);
  });

  useEffect(() => {
    page.current?.querySelector(".keybind.current")?.scrollIntoView({ block: "nearest" });
  }, [selected]);

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

  const addFromLibrary = () => {
    if (!online || !state) return;
    const items: PaletteItem[] = addableServers(state, workspace).map((server) => ({
      id: `add/${server.id}`,
      label: server.name,
      detail: connectorAgent(server)
        ? `${connectorAgent(server)} connector`
        : [mcpSummary(server.transport), server.profiles?.map((p) => p.name).join(", ")]
            .filter(Boolean)
            .join(", "),
      run: () => void addServer(server),
    }));
    items.push({
      id: "add/new",
      label: "New server...",
      detail: "Paste a URL, a command or JSON from the docs",
      always: true,
      run: () => setForm({ kind: "add" }),
    });
    onPicker({
      placeholder: `Add an MCP server to ${workspace.name}`,
      empty: "Every server in the library is here already",
      items,
    });
  };

  const serverActions = ({ server, entry, profile }: WorkspaceServer): RowAction[] => {
    if (!online) return [];
    const actions: RowAction[] = [
      {
        label: entry.enabled ? "Turn off" : "Turn on",
        action: "mcp.toggle",
        run: () => setEntry(server, { ...entry, enabled: !entry.enabled }),
      },
    ];
    if (canHaveProfiles(server)) {
      actions.push(
        { label: "Profile", action: "mcp.profile", run: () => setProfileFor(server.id) },
        {
          label: "New profile",
          action: "mcp.new_profile",
          run: () => setForm({ kind: "profile", server }),
        },
      );
    }
    if (canLogIn(server) && (profile || server.needs_login)) {
      actions.push({
        label: profile?.auth === "logged_in" ? "Log in again" : "Log in",
        action: "mcp.login",
        // A login belongs to a profile, so without one, logging in makes one.
        run: () =>
          profile ? requests.logIn(server, profile, server.id) : setForm({ kind: "profile", server }),
      });
    }
    if (profile && canLogOut(server, profile)) {
      actions.push({
        label: "Log out",
        action: "mcp.logout",
        run: () => requests.logOut(server, profile, server.id),
      });
    }
    if (canEdit(server)) {
      actions.push({ label: "Edit", action: "mcp.edit", run: () => setForm({ kind: "edit", server }) });
    }
    actions.push({
      label: "Remove from workspace",
      action: "mcp.remove",
      run: () => setEntry(server, null, "Removing"),
    });
    return actions;
  };

  const rowActions = (row: PageRow): RowAction[] =>
    row.kind === "topic"
      ? [{ label: "Open", action: "workspace.open", run: () => onOpenTopic(row.topic.id) }]
      : row.kind === "action"
        ? [{ label: row.label, action: "workspace.open", run: row.run }]
        : serverActions(row.item);

  const listActions: RowAction[] = online
    ? [{ label: "Add server", action: "mcp.add", run: addFromLibrary }]
    : [];
  if (online && copySources.length > 0) {
    listActions.push({ label: "Copy from workspace...", action: "mcp.copy_from", run: copyFrom });
  }

  const inTopics = current?.kind === "topic" || (current?.kind === "action" && current.id === "new-topic");
  /** One key for adding, which opens whatever the section the cursor is in adds. */
  const addHere = () => (inTopics ? onNewTopic() : addFromLibrary());

  useHints("pane", [
    ...(current ? rowActions(current).map(({ label, action }) => hint(label, action)) : []),
    // The add key adds to the section the cursor is in.
    hint(inTopics ? "New topic" : "Add server", "mcp.add"),
    ...listActions
      .filter(({ action }) => action !== "mcp.add")
      .map(({ label, action }) => hint(label.replace("...", ""), action)),
    hint("Move", "workspace.up", "workspace.down"),
    hint("First, last", "workspace.first", "workspace.last"),
  ]);

  // Running one from the palette lands on its row, where its key works.
  const paletteItems: PaletteItem[] = listActions.map(({ action, run }) => ({
    id: `workspace/${workspace.id}/${action}`,
    label:
      action === "mcp.add"
        ? `Add an MCP server to ${workspace.name}`
        : `Copy MCP servers into ${workspace.name} from another workspace`,
    keys: keysLabel(action),
    run,
  }));
  for (const [index, row] of rows.entries()) {
    if (row.kind !== "server") continue;
    for (const { label, action, run } of serverActions(row.item)) {
      paletteItems.push({
        id: `workspace/${workspace.id}/${row.item.server.id}/${action}`,
        label: `${label}: ${row.item.server.name}`,
        detail: `MCP server in ${workspace.name}`,
        keys: row === current ? keysLabel(action) : undefined,
        run: () => {
          setCursor(index);
          run();
        },
      });
    }
  }
  useScreenPaletteItems(paletteItems);

  useEffect(() => {
    if (form || profileFor) return;
    const onKey = (event: KeyboardEvent) => {
      if (!inPane(event)) return;
      const moveTo = (index: number) => () =>
        setCursor(Math.max(Math.min(index, rows.length - 1), 0));
      const commands: [ActionId, () => void][] = [
        ["workspace.up", moveTo(selected - 1)],
        ["workspace.down", moveTo(selected + 1)],
        ["workspace.first", moveTo(0)],
        ["workspace.last", moveTo(rows.length - 1)],
        // Enter does what the row is for: going into a topic, or choosing a server's profile.
        [
          "workspace.open",
          () => {
            if (current?.kind === "topic") onOpenTopic(current.topic.id);
            else if (current?.kind === "action") current.run();
            else if (current && online && canHaveProfiles(current.item.server)) {
              setProfileFor(current.item.server.id);
            }
          },
        ],
        ...listActions.map(({ action, run }): [ActionId, () => void] =>
          action === "mcp.add" ? [action, addHere] : [action, run],
        ),
      ];
      if (commands.some(([id, run]) => act(event, id, run))) return;
      if (current?.kind === "server") actOnServer(event, serverActions(current.item));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const pointAt = (index: number) => (event: MouseEvent<HTMLElement>) => {
    // Moving, rather than entering: a row scrolling under a pointer that isn't moving must
    // not take the selection.
    if (event.movementX === 0 && event.movementY === 0) return;
    setCursor(index);
  };

  const topicRow = (topic: Topic, index: number) => {
    const status = topicState(topic, online);
    const tabs = tabsOf(topic).length;
    // Idle is what the icon already says, so only what needs a look gets words.
    const loud = status === "waiting" || status === "failed";
    return (
      <button
        key={topic.id}
        className={`keybind workspace-row ${index === selected ? "current" : ""}`}
        tabIndex={-1}
        onMouseMove={pointAt(index)}
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

  const actionRow = (row: Extract<PageRow, { kind: "action" }>, index: number) => (
    <button
      key={row.id}
      className={`keybind workspace-row workspace-empty ${index === selected ? "current" : ""}`}
      tabIndex={-1}
      disabled={!online}
      onMouseMove={pointAt(index)}
      onClick={row.run}
    >
      <Icon name={row.icon} />
      <span className="name">{row.label}</span>
    </button>
  );

  const serverRow = (item: WorkspaceServer, index: number) => {
    const { server, entry, profile } = item;
    const isCurrent = index === selected;
    const busy = requests.busy[server.id];
    const auth = entryAuth(server, profile);
    const agent = connectorAgent(server);
    const hasProfiles = (server.profiles?.length ?? 0) > 0;
    return (
      <div
        key={server.id}
        className={`keybind workspace-row ${isCurrent ? "current" : ""} ${entry.enabled ? "" : "off"}`}
        data-server={server.id}
        onMouseMove={pointAt(index)}
      >
        <span className="server-name">{server.name}</span>
        <span className="server-summary code">{agent ? `${agent} connector` : mcpSummary(server.transport)}</span>
        {busy ? (
          <span className="meta">{busy}</span>
        ) : requests.errors[server.id] ? (
          <span className="meta bad" title={requests.errors[server.id]}>
            Failed
          </span>
        ) : (
          auth === "needs_login" &&
          online && (
            <button
              className="picker alert"
              tabIndex={-1}
              title={withKey("Log in", "mcp.login")}
              onMouseDown={(event) => event.preventDefault()}
              onClick={() =>
                profile ? requests.logIn(server, profile, server.id) : setForm({ kind: "profile", server })
              }
            >
              Log in
            </button>
          )
        )}
        {canHaveProfiles(server) && (
          <Picker
            label={`Profile ${server.name} uses`}
            // Without profiles there's nothing to choose yet, so it only shows on the row in use.
            className={`picker ${hasProfiles ? "" : "row-quiet"}`}
            value={profile?.id ?? NONE}
            disabled={!online}
            open={profileFor === server.id}
            onOpenChange={(open) => setProfileFor(open ? server.id : null)}
            onChange={(value) => {
              setProfileFor(null);
              if (value === NEW_PROFILE) setForm({ kind: "profile", server });
              else setEntry(server, { ...entry, profile_id: value === NONE ? undefined : value });
            }}
            finalFocus={page}
            trigger={
              <>
                <span className={profile ? "" : "dim"}>{profile ? profile.name : "Profile"}</span>
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
        <button
          className="switch"
          tabIndex={-1}
          role="switch"
          aria-checked={entry.enabled}
          aria-label={`${server.name} on in ${workspace.name}`}
          title={withKey(entry.enabled ? "Turn off" : "Turn on", "mcp.toggle")}
          disabled={!online}
          onMouseDown={(event) => event.preventDefault()}
          onClick={() => setEntry(server, { ...entry, enabled: !entry.enabled })}
        />
        <MenuButton
          label={`Actions for ${server.name}`}
          className="icon-button row-menu"
          groups={actionMenu(serverActions(item))}
        >
          <Icon name="more" />
        </MenuButton>
      </div>
    );
  };

  const serverMenuAt = (target: Element): MenuGroups => {
    const id = target.closest<HTMLElement>("[data-server]")?.dataset.server;
    const item = servers.find((s) => s.server.id === id);
    return item ? actionMenu(serverActions(item)) : [];
  };

  const renderRow = (row: PageRow, index: number) =>
    row.kind === "topic"
      ? topicRow(row.topic, index)
      : row.kind === "server"
        ? serverRow(row.item, index)
        : actionRow(row, index);

  const pageMenu: MenuGroups = online
    ? [
        [
          { id: "new-topic", label: "New topic", keys: keysLabel("global.new_topic"), run: onNewTopic },
          ...listActions.map(({ action, run }) => ({
            id: action,
            label: action === "mcp.add" ? "Add MCP server" : "Copy MCP servers from...",
            keys: keysLabel(action),
            run,
          })),
        ],
      ]
    : [];

  const section = (title: string, count: number, add: { label: string; action: ActionId; run: () => void }) => (
    <div className="workspace-section-head">
      <h2 className="workspace-section-title">{title}</h2>
      {count > 0 && <span className="workspace-count">{count}</span>}
      <button
        className="icon-button"
        disabled={!online}
        title={withKey(add.label, add.action)}
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
        tabIndex={-1}
        // A click anywhere on the page gives it the keyboard, which is what shows the selection.
        onMouseDown={(event) => {
          if (!event.currentTarget.contains(document.activeElement)) {
            event.currentTarget.focus({ preventScroll: true });
          }
        }}
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
              action: "global.new_topic",
              run: onNewTopic,
            })}
            <div className="keybinds">{topicRows.map((row, index) => renderRow(row, index))}</div>
            {/* Restoring from here comes later. It's here so the page has its place. */}
            <button className="workspace-link" tabIndex={-1} title="Not built yet">
              Archived topics
            </button>
          </section>
          <section className="workspace-section">
            {section("MCP servers", servers.length, {
              label: "Add MCP server",
              action: "mcp.add",
              run: addFromLibrary,
            })}
            <RightClickMenu render={<div className="keybinds" />} groupsAt={serverMenuAt}>
              {serverRows.map((row, index) => renderRow(row, topicRows.length + index))}
            </RightClickMenu>
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
