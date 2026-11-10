import type { KeyboardEvent } from "react";
import type { Device, Workspace } from "./engine";
import type { View } from "./navigation";
import { act } from "./keybinds";
import { ago, isOnline, STATE_LABEL, topicState } from "./model";
import { Icon } from "./ui/Icon";
import { MenuButton, type MenuGroups, RightClickMenu } from "./ui/Menu";
import { Keys, StatusIcon } from "./ui/StatusIcon";

interface Props {
  devices: Device[];
  view: View | null;
  onView: (view: View | null) => void;
  onPreview: (view: View | null) => void;
  onPalette: () => void;
  /** What isn't about one topic, behind the button next to Settings. */
  appMenu: MenuGroups;
  /** What a right click on a topic offers. */
  topicMenu: (machineId: string, topicId: string) => MenuGroups;
  onSettings: (machineId: string) => void;
  onAppSettings: () => void;
  onAddWorkspace: (device: Device) => void;
  onNewTopic: (device: Device, workspace: Workspace) => void;
}

function deviceMeta(device: Device): string | null {
  switch (device.connection.state) {
    case "online":
      return null;
    case "connecting":
      return "Connecting";
    case "incompatible":
      return "Needs an update";
    case "offline":
      return device.last_seen_ms ? `Offline, ${ago(device.last_seen_ms)}` : "Offline";
  }
}

/** The view a row shows as soon as the keyboard reaches it. */
function viewOf(row: HTMLElement): View | null {
  const { machine, topic } = row.dataset;
  return machine && topic ? { kind: "topic", machineId: machine, topicId: topic } : null;
}

/** Walks every row marked `data-nav`, opening what it lands on while the sidebar keeps the
 * keyboard. Confirming on a device opens its settings. Adding is handled in App. */
function onSidebarKey(event: KeyboardEvent<HTMLElement>, onPreview: (view: View | null) => void) {
  // A menu opened from here is drawn elsewhere on the page, but its keys still come up
  // through here, and they're the menu's.
  if (!(event.target instanceof Node) || !event.currentTarget.contains(event.target)) return;
  const row = event.target as HTMLElement;
  const rows = [...event.currentTarget.querySelectorAll<HTMLElement>("[data-nav]")];
  const index = rows.indexOf(row);
  const moveTo = (next: () => HTMLElement | null | undefined) => () => {
    const target = next();
    if (!target) return;
    target.focus();
    const shown = viewOf(target);
    if (shown) onPreview(shown);
  };
  const parent = () =>
    row.matches(".topic")
      ? row.parentElement?.querySelector<HTMLElement>(".row.workspace[data-nav]")
      : row.matches(".workspace")
        ? row.closest("section")?.querySelector<HTMLElement>(".row.device[data-nav]")
        : null;
  const confirm = () => {
    if (row.matches(".device")) row.querySelector<HTMLElement>(".gear")?.click();
    else row.click();
  };
  // Half of however many rows fit, like Ctrl+D and Ctrl+U move half a screen in Vim.
  const halfScreen = () => {
    const shown = (row.closest(".tree")?.clientHeight ?? 0) / (row.offsetHeight || 1);
    return Math.max(Math.floor(shown / 2), 1);
  };
  act(event, "sidebar.down", moveTo(() => rows[index + 1])) ||
    act(event, "sidebar.half_up", moveTo(() => rows[Math.max(index - halfScreen(), 0)])) ||
    act(
      event,
      "sidebar.half_down",
      moveTo(() => rows[Math.min(index + halfScreen(), rows.length - 1)]),
    ) ||
    act(event, "sidebar.center", () => row.scrollIntoView({ block: "center" })) ||
    act(event, "sidebar.up", moveTo(() => rows[index - 1])) ||
    act(event, "sidebar.first", moveTo(() => rows[0])) ||
    act(event, "sidebar.last", moveTo(() => rows[rows.length - 1])) ||
    act(event, "sidebar.parent", moveTo(parent)) ||
    act(event, "sidebar.open", () => {
      if (viewOf(row)) row.click();
    }) ||
    act(event, "sidebar.confirm", confirm);
}

export function Sidebar({
  devices,
  view,
  onView,
  onPreview,
  onPalette,
  appMenu,
  topicMenu,
  onSettings,
  onAppSettings,
  onAddWorkspace,
  onNewTopic,
}: Props) {
  const menuAt = (target: Element): MenuGroups => {
    const { machine, topic } = target.closest<HTMLElement>(".row.topic")?.dataset ?? {};
    return machine && topic ? topicMenu(machine, topic) : [];
  };
  return (
    <aside className="side" onKeyDown={(event) => onSidebarKey(event, onPreview)}>
      {/* Looks like the box it opens, so the palette is somewhere to go and not only a key. */}
      <button
        className="row side-search"
        tabIndex={-1}
        // Focus stays where it was, which is where closing the palette puts it back.
        onMouseDown={(event) => event.preventDefault()}
        onClick={onPalette}
      >
        <Icon name="search" />
        <span className="name">Find a topic or command</span>
        <Keys action="global.palette" />
      </button>
      <RightClickMenu render={<nav className="tree" />} groupsAt={menuAt}>
        {devices.map((device) => {
          const online = isOnline(device);
          const machineId = device.state?.machine.id;
          const name = device.state?.machine.name ?? device.url;
          const refusal = `${name} is offline, so nothing can be created there`;
          return (
            <section key={device.url}>
              <div
                className={`row device ${online ? "" : "off"}`}
                title={offlineReason(device)}
                data-nav={online ? "" : undefined}
                tabIndex={-1}
              >
                <span className={`dot ${online ? "" : "off"}`} />
                <span className="name">{name}</span>
                {deviceMeta(device) && <span className="meta">{deviceMeta(device)}</span>}
                <button
                  className="gear"
                  disabled={!online}
                  tabIndex={-1}
                  title={online ? "Device settings" : `${name} is offline`}
                  aria-label={`Settings for ${name}`}
                  onClick={() => machineId && onSettings(machineId)}
                >
                  <Icon name="settings" />
                </button>
                <button
                  className="add"
                  disabled={!online}
                  tabIndex={-1}
                  title={online ? "Add a workspace" : refusal}
                  aria-label={`Add a workspace on ${name}`}
                  onClick={() => onAddWorkspace(device)}
                >
                  <Icon name="add" />
                </button>
              </div>
              {online && device.state?.workspaces.length === 0 && (
                <button
                  className="row empty"
                  data-nav
                  tabIndex={-1}
                  onClick={() => onAddWorkspace(device)}
                >
                  <span className="name">No workspaces yet</span>
                  <Keys action="global.new_workspace" />
                </button>
              )}
              {device.state?.workspaces.map((workspace) => (
                <div key={workspace.id}>
                  <div
                    className="row workspace"
                    title={workspace.path}
                    data-nav={online ? "" : undefined}
                    tabIndex={-1}
                  >
                    <span className="name">{workspace.name}</span>
                    <button
                      className="add"
                      disabled={!online}
                      tabIndex={-1}
                      title={online ? "Create a topic" : refusal}
                      aria-label={`Create a topic in ${workspace.name}`}
                      onClick={() => onNewTopic(device, workspace)}
                    >
                      <Icon name="add" />
                    </button>
                  </div>
                  {online && workspace.topics.length === 0 && (
                    <button
                      className="row empty"
                      data-nav
                      tabIndex={-1}
                      onClick={() => onNewTopic(device, workspace)}
                    >
                      <span className="name">No topics yet</span>
                      <Keys action="global.new_topic" />
                    </button>
                  )}
                  {workspace.topics.map((topic) => {
                    const state = topicState(topic, online);
                    const current =
                      view?.kind === "topic" &&
                      view.machineId === machineId &&
                      view.topicId === topic.id;
                    return (
                      <button
                        key={topic.id}
                        className={`row topic ${state} ${current ? "current" : ""}`}
                        data-nav
                        data-machine={machineId}
                        data-topic={topic.id}
                        tabIndex={current ? 0 : -1}
                        onClick={() =>
                          machineId && onView({ kind: "topic", machineId, topicId: topic.id })
                        }
                      >
                        <StatusIcon state={state} />
                        <span className="name">{topic.name}</span>
                        {(state === "waiting" || state === "failed") && (
                          <span className={`meta ${state === "waiting" ? "alert" : "bad"}`}>
                            {STATE_LABEL[state]}
                          </span>
                        )}
                      </button>
                    );
                  })}
                </div>
              ))}
            </section>
          );
        })}
      </RightClickMenu>
      <div className="side-foot">
        <button className="row" tabIndex={-1} onClick={onAppSettings}>
          <span className="name">Settings</span>
          <Keys action="global.settings" />
        </button>
        <MenuButton label="More actions" className="icon-button" groups={appMenu}>
          <Icon name="more" />
        </MenuButton>
      </div>
    </aside>
  );
}

function offlineReason(device: Device): string | undefined {
  const { connection } = device;
  if (connection.state === "offline") return connection.reason;
  if (connection.state === "incompatible") {
    return `The daemon speaks protocol ${connection.daemon_min} to ${connection.daemon_max}, which this app doesn't`;
  }
  return undefined;
}
