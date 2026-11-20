import { useEffect, useState } from "react";
import {
  type ClosedWorkspace,
  type CommandOutput,
  type Device,
  errorText,
  type RepoClone,
  request,
  type Workspace,
} from "./engine";
import { KeepAwakeFor } from "./KeepAwake";
import {
  archiveLosses,
  cloneProgress,
  connectionProblem,
  deviceName,
  deviceStatus,
  homeShort,
  isOnline,
  STATE_LABEL,
  workspaceState,
} from "./model";
import type { SheetRequest } from "./Sheets";
import { Icon } from "./ui/Icon";
import { MenuButton, type MenuGroups, RightClickMenu } from "./ui/Menu";
import { onRowKeys, SettingRow, useRovingList } from "./ui/SettingsPage";
import { StatusIcon } from "./ui/StatusIcon";
import { useNow } from "./ui/useNow";

const counted = (count: number, thing: string) => `${count} ${thing}${count === 1 ? "" : "s"}`;

const closedOn = (ms: number) =>
  new Date(ms).toLocaleDateString(undefined, { month: "short", day: "numeric" });

/** A device's own page: how it is, keeping it awake, its workspaces, and the closed ones, which
 * expand in place under the open ones like a workspace's archived topics. The workspaces are one
 * Tab stop with the arrows moving through them, and each one's menu is a Tab away. */
export function DevicePage({
  device,
  workspaceMenu,
  onOpenWorkspace,
  onAddWorkspace,
  onSettings,
  onConfirm,
}: {
  device: Device;
  /** What a workspace offers, for its row's menu and a right click on it. */
  workspaceMenu: (workspaceId: string) => MenuGroups;
  onOpenWorkspace: (workspaceId: string) => void;
  onAddWorkspace: () => void;
  onSettings: () => void;
  onConfirm: (sheet: SheetRequest) => void;
}) {
  const state = device.state;
  const machineId = state?.machine.id;
  const online = isOnline(device);
  const name = deviceName(device);
  const workspaces = state?.workspaces ?? [];
  const clones = state?.clones ?? [];
  const now = useNow(state?.awake_until_ms !== undefined);
  const status = deviceStatus(device, now);
  const [error, setError] = useState<string | null>(null);
  // Null while they're being asked for.
  const [closed, setClosed] = useState<ClosedWorkspace[] | null>(null);
  const [closedError, setClosedError] = useState<string | null>(null);
  // Whether the closed ones are open under the list. Until someone says, they are while the
  // device has no open workspaces, which is when they're what the page is for.
  const [closedChoice, setClosedChoice] = useState<boolean | null>(null);
  const closedOpen = closedChoice ?? workspaces.length === 0;
  // What's being done to a closed workspace, by its ID.
  const [busy, setBusy] = useState<Record<string, string>>({});
  const [rowErrors, setRowErrors] = useState<Record<string, string>>({});
  const rows = useRovingList(Math.max(workspaces.length, 1));

  // Asked for again whenever a workspace opens or closes, which can happen on any client.
  const openIds = workspaces.map((w) => w.id).join(" ");
  useEffect(() => {
    if (!machineId || !online) return;
    let shown = true;
    setClosedError(null);
    request(machineId, { command: "list_closed_workspaces" })
      .then((output) => {
        if (shown) setClosed(output.output === "closed_workspaces" ? output.workspaces : []);
      })
      .catch((reason) => {
        if (shown) setClosedError(errorText(reason));
      });
    return () => {
      shown = false;
    };
  }, [machineId, online, openIds]);

  const forgetRow = (id: string) => {
    setClosed((now) => now?.filter((w) => w.id !== id) ?? null);
    setBusy(({ [id]: _, ...rest }) => rest);
  };

  const openClosed = async (workspace: ClosedWorkspace) => {
    if (!machineId || !online || busy[workspace.id]) return;
    setBusy((now) => ({ ...now, [workspace.id]: "Opening" }));
    setRowErrors(({ [workspace.id]: _, ...rest }) => rest);
    try {
      await request(machineId, { command: "open_workspace", workspace_id: workspace.id });
      forgetRow(workspace.id);
      onOpenWorkspace(workspace.id);
    } catch (reason) {
      setBusy(({ [workspace.id]: _, ...rest }) => rest);
      setRowErrors((now) => ({ ...now, [workspace.id]: errorText(reason) }));
    }
  };

  /** Asks once, since it can't be undone, and again with exactly what goes when git would lose
   * work too. */
  const remove = (workspace: ClosedWorkspace) => {
    if (!machineId || !online) return;
    const afterRemoving = (output: CommandOutput) => {
      if (output.output !== "archive_risk") {
        forgetRow(workspace.id);
        return;
      }
      onConfirm({
        kind: "confirm",
        machineId,
        where: `Removing ${workspace.name} loses this`,
        hint: `${archiveLosses(output.risk).join(". ")}. It can't be brought back.`,
        action: "Remove and lose it",
        command: { command: "remove_workspace", workspace_id: workspace.id, force: true },
        then: () => forgetRow(workspace.id),
      });
    };
    onConfirm({
      kind: "confirm",
      machineId,
      where: `Remove ${workspace.name}`,
      hint: "Its topics, tabs and transcripts are deleted, and the worktrees tectr made. The repo stays.",
      action: "Remove workspace",
      command: { command: "remove_workspace", workspace_id: workspace.id },
      then: afterRemoving,
    });
  };

  const pageMenu: MenuGroups = [
    [
      ...(online ? [{ id: "add-workspace", label: "Add a workspace", run: onAddWorkspace }] : []),
      { id: "device-settings", label: "Device settings", run: onSettings },
    ],
  ];

  const workspaceRow = (workspace: Workspace, index: number) => {
    const { state: urgent, count } = workspaceState(workspace, online);
    // Idle and finished are what a quiet row already says, so only what needs a look gets words.
    const loud = urgent === "waiting" || urgent === "failed" || urgent === "working";
    const menu = online ? workspaceMenu(workspace.id) : [];
    return (
      <div
        key={workspace.id}
        className="device-workspace"
        data-machine={machineId}
        data-workspace={workspace.id}
      >
        <button
          className="setting-line workspace-topic"
          {...rows.item(index)}
          data-setting=""
          title={workspace.path}
          onClick={() => onOpenWorkspace(workspace.id)}
        >
          <StatusIcon state={loud ? urgent : online ? "idle" : "stale"} />
          <span className="name">{workspace.name}</span>
          <span className="device-path code">{homeShort(workspace.path)}</span>
          {loud && (
            <span className={`meta ${urgent === "waiting" ? "alert" : urgent === "failed" ? "bad" : ""}`}>
              {count > 1 ? `${count} ${STATE_LABEL[urgent].toLowerCase()}` : STATE_LABEL[urgent]}
            </span>
          )}
          <span className="meta">{counted(workspace.topics.length, "topic")}</span>
        </button>
        {menu.length > 0 && (
          // A Tab after its row, and only its row's, so Tab doesn't walk every workspace's menu.
          <span className="device-row-menu">
            <MenuButton
              label={`Actions for ${workspace.name}`}
              className="icon-button"
              groups={menu}
              tabIndex={index === rows.current ? 0 : -1}
            >
              <Icon name="more" />
            </MenuButton>
          </span>
        )}
      </div>
    );
  };

  const closedRows = () => {
    if (closedError) return <p className="error workspace-error">{closedError}</p>;
    if (closed === null) return <p className="settings-note workspace-note">Loading</p>;
    if (closed.length === 0) return <p className="settings-note workspace-note">No closed workspaces</p>;
    return (
      <div className="settings-rows">
        {closed.map((workspace) => {
          const doing = busy[workspace.id];
          return (
            <SettingRow
              key={workspace.id}
              label={workspace.name}
              detail={homeShort(workspace.path)}
              dim
              meta={
                <span className="meta">
                  {doing ?? `${counted(workspace.topics, "topic")}, closed ${closedOn(workspace.closed_at_ms)}`}
                </span>
              }
              error={rowErrors[workspace.id]}
              quiet={
                <MenuButton
                  label={`Actions for ${workspace.name}`}
                  className="icon-button"
                  disabled={!online || doing !== undefined}
                  groups={[[{ id: "remove", label: "Remove workspace...", run: () => remove(workspace) }]]}
                >
                  <Icon name="more" />
                </MenuButton>
              }
            >
              <button
                className="button small"
                disabled={!online || doing !== undefined}
                onClick={() => void openClosed(workspace)}
              >
                Open
              </button>
            </SettingRow>
          );
        })}
      </div>
    );
  };

  // A clone that fails again says so in its own row, through the daemon, so there's nothing to
  // catch here. Dismissing only fails when it's already gone.
  const cloneAgain = (clone: RepoClone) => {
    if (!machineId || !online) return;
    request(machineId, { command: "clone_workspace", url: clone.url, path: clone.path }).catch(() => {});
  };
  const dismissClone = (clone: RepoClone) => {
    if (!machineId || !online) return;
    request(machineId, { command: "dismiss_clone", path: clone.path }).catch(() => {});
  };

  const cloneRows = () => (
    <div className="settings-rows">
      {clones.map((clone) => {
        const progress = cloneProgress(clone);
        return (
          <SettingRow
            key={clone.path}
            label={clone.name}
            detail={homeShort(clone.path)}
            dim
            meta={<span className={progress.failed ? "meta bad" : "meta"}>{progress.text}</span>}
            error={clone.error}
          >
            {progress.failed && (
              <>
                <button className="button small" disabled={!online} onClick={() => dismissClone(clone)}>
                  Dismiss
                </button>
                <button className="button small" disabled={!online} onClick={() => cloneAgain(clone)}>
                  Try again
                </button>
              </>
            )}
          </SettingRow>
        );
      })}
    </div>
  );

  const workspaceMenuAt = (target: Element): MenuGroups => {
    const id = target.closest<HTMLElement>("[data-workspace]")?.dataset.workspace;
    return id && online ? workspaceMenu(id) : [];
  };

  return (
    <div
      className="pane settings workspace-page"
      // Where the keyboard lands when the page opens, so Tab and the row keys start from the top.
      // Not a Tab stop. Esc goes back to the sidebar, which App does for every pane.
      tabIndex={-1}
      onKeyDown={(event) => onRowKeys(event)}
    >
      <div className="settings-in workspace-in">
        <header className="workspace-head">
          <div className="workspace-heading">
            <h1 className="workspace-title">{name}</h1>
            <p className="workspace-sub" title={connectionProblem(device)}>
              <span className={status?.alert ? "meta alert" : undefined}>{status?.text ?? "Online"}</span>
            </p>
          </div>
          {state && (
            <span className="device-awake">
              <span>Keep awake</span>
              <KeepAwakeFor device={device} onError={setError} />
            </span>
          )}
          <MenuButton label={`Actions for ${name}`} className="icon-button" groups={pageMenu}>
            <Icon name="more" />
          </MenuButton>
        </header>
        {error && <p className="error workspace-error">{error}</p>}
        <section className="workspace-section">
          <div className="workspace-section-head">
            <h2 className="workspace-section-title">Workspaces</h2>
            {workspaces.length > 0 && <span className="workspace-count">{workspaces.length}</span>}
            <button
              className="icon-button"
              disabled={!online}
              title="Add a workspace"
              aria-label={`Add a workspace on ${name}`}
              onClick={onAddWorkspace}
            >
              <Icon name="add" />
            </button>
          </div>
          {workspaces.length > 0 ? (
            <RightClickMenu render={<div className="settings-rows" />} groupsAt={workspaceMenuAt}>
              {workspaces.map(workspaceRow)}
            </RightClickMenu>
          ) : (
            <div className="settings-rows">
              <button
                className="setting-line workspace-empty"
                data-setting=""
                disabled={!online}
                onClick={onAddWorkspace}
              >
                <Icon name="add" />
                <span className="name">Add a workspace</span>
              </button>
            </div>
          )}
          {clones.length > 0 && cloneRows()}
          <button
            className="workspace-disclosure"
            aria-expanded={closedOpen}
            data-setting=""
            disabled={!online}
            onClick={() => setClosedChoice(!closedOpen)}
          >
            <Icon name={closedOpen ? "fold" : "unfold"} />
            <span>Closed workspaces</span>
            {closed !== null && closed.length > 0 && <span className="workspace-count">{closed.length}</span>}
          </button>
          {closedOpen && online && closedRows()}
        </section>
      </div>
    </div>
  );
}
