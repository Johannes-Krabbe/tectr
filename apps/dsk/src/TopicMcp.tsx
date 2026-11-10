import { Dialog } from "@base-ui/react/dialog";
import { type KeyboardEvent, useEffect, useRef, useState } from "react";
import type { DaemonState, McpServer, Topic } from "./engine";
import { act, type ActionId, remapNative } from "./keybinds";
import {
  canLogIn,
  canLogOut,
  connectorAgent,
  connectors,
  deviceServers,
  flippedTopicMcp,
  mcpAuthLabel,
  mcpSummary,
  ownServers,
  takenNames,
  topicGets,
  topicMcpChoice,
  topicMcpLabel,
} from "./mcp";
import { McpAddSheet } from "./McpAdd";
import { actOnServer, McpServerSheet, serverActions, useMcpRequests } from "./McpServers";
import { hint, useHints } from "./shortcuts";
import { Keys } from "./ui/StatusIcon";

const BACK_KEY: [ActionId, string][] = [["mcp.back", "Escape"]];

type RowAction = { label: string; action: ActionId; run: () => void };

/** Which MCP servers a topic's agents get: the device's and its agents' connectors, each
 * following its default or turned on or off here, and the topic's own. */
export function TopicMcpSheet({
  state,
  topic,
  online,
  onClose,
}: {
  state: DaemonState;
  topic: Topic;
  online: boolean;
  onClose: () => void;
}) {
  const machineId = state.machine.id;
  const fromDevice = deviceServers(state);
  const connected = connectors(state);
  const own = ownServers(state, topic.id);
  const rows = [...fromDevice, ...connected, ...own];
  const requests = useMcpRequests(machineId);
  const [cursor, setCursor] = useState(0);
  // A topic's own server being added or changed. Undefined adds one.
  const [form, setForm] = useState<{ server?: McpServer } | null>(null);
  const list = useRef<HTMLDivElement>(null);

  const selected = Math.max(Math.min(cursor, rows.length - 1), 0);
  const current = rows[selected];

  const setOn = (server: McpServer, on: boolean | null) =>
    void requests.run(server.id, "Saving", {
      command: "set_topic_mcp",
      topic_id: topic.id,
      server_id: server.id,
      on,
    });

  const actionsOf = (server: McpServer): RowAction[] => {
    if (!online) return [];
    if (server.topic_id) {
      const edit = (target: McpServer) => setForm({ server: target });
      return serverActions(server, requests, edit, server.enabled ? "Turn off" : "Turn on");
    }
    const choice = topicMcpChoice(topic, server);
    const actions: RowAction[] = [
      {
        label: topicGets(topic, server) ? "Turn off" : "Turn on",
        action: "mcp.toggle",
        run: () => setOn(server, flippedTopicMcp(topic, server)),
      },
    ];
    if (choice !== "on") {
      actions.push({ label: "On here", action: "mcp.turn_on", run: () => setOn(server, true) });
    }
    if (choice !== "off") {
      actions.push({ label: "Off here", action: "mcp.turn_off", run: () => setOn(server, false) });
    }
    if (choice !== "default") {
      actions.push({
        label: "Follow default",
        action: "mcp.follow_default",
        run: () => setOn(server, null),
      });
    }
    if (canLogIn(server)) {
      actions.push({ label: "Log in", action: "mcp.login", run: () => requests.logIn(server) });
    }
    if (canLogOut(server)) {
      actions.push({ label: "Log out", action: "mcp.logout", run: () => requests.logOut(server) });
    }
    return actions;
  };

  const currentActions = current ? actionsOf(current) : [];

  useHints(
    "overlay",
    form
      ? null
      : [
          ...currentActions.map(({ label, action }) => hint(label, action)),
          ...(online ? [hint("Add", "mcp.add")] : []),
          ...(rows.length > 1
            ? [hint("Move", "mcp.up", "mcp.down"), hint("First, last", "mcp.first", "mcp.last")]
            : []),
          hint("Close", "mcp.back"),
        ],
  );

  useEffect(() => {
    list.current?.querySelector("[data-highlighted]")?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    // The form is inside this sheet, and its keys bubble through here.
    if (form) return;
    const moveTo = (index: number) => () => setCursor(Math.max(Math.min(index, rows.length - 1), 0));
    const commands: [ActionId, () => void][] = [
      ["mcp.up", moveTo(selected - 1)],
      ["mcp.down", moveTo(selected + 1)],
      ["mcp.first", moveTo(0)],
      ["mcp.last", moveTo(rows.length - 1)],
      ["mcp.add", () => online && setForm({})],
    ];
    if (commands.some(([id, run]) => act(event, id, run))) return;
    actOnServer(event, currentActions);
  };

  const row = (server: McpServer) => {
    const index = rows.indexOf(server);
    const highlighted = index === selected;
    const auth = mcpAuthLabel(server);
    const doing = requests.busy[server.id];
    return (
      <div
        key={server.id}
        className="palette-item server-row"
        role="option"
        aria-selected={highlighted}
        data-highlighted={highlighted ? "" : undefined}
        onMouseMove={() => setCursor(index)}
      >
        <span className="server-name code">{server.name}</span>
        <span className="server-summary code">{mcpSummary(server.transport)}</span>
        {doing && <span className="meta">{doing}</span>}
        {connectorAgent(server) && <span className="meta">{connectorAgent(server)}</span>}
        {auth && <span className={`meta ${server.auth === "needs_login" ? "alert" : ""}`}>{auth}</span>}
        <span className="meta">
          {server.topic_id ? (server.enabled ? "On" : "Off") : topicMcpLabel(topic, server)}
        </span>
      </div>
    );
  };

  const error = current && requests.errors[current.id];

  return (
    <Dialog.Root open onOpenChange={(open) => !open && !form && onClose()}>
      <Dialog.Portal>
        <Dialog.Backdrop className="scrim" />
        <Dialog.Popup
          className="sheet"
          initialFocus={list}
          onKeyDownCapture={(event) => {
            if (!form) remapNative(event, BACK_KEY);
          }}
          onKeyDown={onKeyDown}
        >
          <Dialog.Title className="sheet-where">MCP servers for {topic.name}</Dialog.Title>
          <div
            ref={list}
            className="palette-list"
            role="listbox"
            aria-label="MCP servers"
            tabIndex={-1}
          >
            <p className="sheet-group">From {state.machine.name}</p>
            {fromDevice.length === 0 && <p className="sheet-group-empty">None</p>}
            {fromDevice.map(row)}
            {connected.length > 0 && <p className="sheet-group">Connectors</p>}
            {connected.map(row)}
            <p className="sheet-group">Only this topic</p>
            {own.length === 0 && <p className="sheet-group-empty">None</p>}
            {own.map(row)}
          </div>
          {error && <p className="error sheet-error">{error}</p>}
          {current && currentActions.length > 0 && (
            <div className="settings-actions">
              <span className="settings-actions-what code">{current.name}</span>
              {currentActions.map(({ label, action, run }) => (
                <button
                  key={action}
                  type="button"
                  className="button"
                  tabIndex={-1}
                  disabled={requests.busy[current.id] !== undefined}
                  onMouseDown={(event) => event.preventDefault()}
                  onClick={run}
                >
                  {label}
                  <Keys action={action} />
                </button>
              ))}
            </div>
          )}
          <div className="sheet-foot">
            <span className="sheet-hint">
              {online ? "Working agents get changes after their turn" : `${state.machine.name} is offline`}
            </span>
            <button
              type="button"
              className="button"
              tabIndex={-1}
              disabled={!online}
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => setForm({})}
            >
              Add server
              <Keys action="mcp.add" />
            </button>
            <Dialog.Close className="button" tabIndex={-1}>
              Close
              <Keys action="mcp.back" />
            </Dialog.Close>
          </div>
          {form?.server && (
            <McpServerSheet
              machineId={machineId}
              server={form.server}
              topicId={topic.id}
              where={`Edit ${form.server.name} in ${topic.name}`}
              onClose={() => setForm(null)}
            />
          )}
          {form && !form.server && (
            <McpAddSheet
              machineId={machineId}
              topicId={topic.id}
              taken={takenNames(state.mcp_servers ?? [], topic.id)}
              where={`New MCP server for ${topic.name}`}
              onClose={() => setForm(null)}
            />
          )}
        </Dialog.Popup>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
