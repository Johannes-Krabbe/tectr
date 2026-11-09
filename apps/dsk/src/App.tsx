import { useEffect, useRef, useState } from "react";
import { AgentPane } from "./AgentPane";
import { shownValue } from "./appearance";
import { AppSettings } from "./AppSettings";
import { DeviceSettings } from "./DeviceSettings";
import {
  type Approval,
  type CommandOutput,
  type Device,
  errorText,
  request,
  type Session,
  useDevices,
  type Workspace,
} from "./engine";
import {
  act,
  type ActionId,
  keysLabel,
  showsSteps,
  titled,
  toggleSteps,
  useSettings,
} from "./keybinds";
import {
  focusPane,
  focusSidebar,
  inPane,
  inSidebar,
  overlayOpen,
  pointedRow,
  pressAdd,
} from "./keys";
import {
  allTopics,
  isOnline,
  sessionLabel,
  sessionState,
  sessionWorktree,
  type TopicEntry,
  topicState,
} from "./model";
import {
  chooseTab,
  goTo,
  landingAfterDeleting,
  landingIn,
  loadNavigation,
  type Navigation,
  saveNavigation,
  shownTab,
  type View,
} from "./navigation";
import { NewTopic, type NewTopicForm } from "./NewTopic";
import { Palette, type PaletteItem } from "./Palette";
import { isLoopback } from "./pathInput";
import { Sheets, type SheetRequest } from "./Sheets";
import { globalHints, pinnedHints, useHints } from "./shortcuts";
import { Sidebar } from "./Sidebar";
import { ShortcutBar } from "./ui/ShortcutBar";
import { Keys, StatusIcon } from "./ui/StatusIcon";

interface RequestItem {
  approval: Approval;
  machineId: string;
  entry: TopicEntry | undefined;
}

const isActive = (session: Session) =>
  session.status === "starting" ||
  session.status === "working" ||
  session.status === "waiting_for_you";

export function App() {
  const devices = useDevices();
  // Rendering again when the settings change is what updates every key shown below.
  useSettings();
  // What exists comes from the daemon, so every client sees the same. What this client is
  // looking at is its own, and the rest is about this keyboard and this moment.
  const [navigation, setNavigation] = useState<Navigation>(loadNavigation);
  const [newTopicForm, setNewTopicForm] = useState<NewTopicForm>({});
  const [sheet, setSheet] = useState<SheetRequest | null>(null);
  const [palette, setPalette] = useState(false);
  // The session just started here, which opens ready for a message.
  const [composeIn, setComposeIn] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Where the keyboard goes once the screen it was waiting for has rendered.
  const focusWhenShown = useRef<(() => void) | null>(null);
  // The tabs each topic last showed, which says where to go when the shown one is closed.
  const seenTabs = useRef<Record<string, string[]>>({});

  const { view } = navigation;

  const topics = allTopics(devices);
  const settingsDevice =
    view?.kind === "device_settings"
      ? devices.find((d) => d.state?.machine.id === view.machineId)
      : undefined;
  const entry =
    view?.kind === "topic"
      ? topics.find((t) => t.machineId === view.machineId && t.topic.id === view.topicId)
      : undefined;
  const sessions = entry?.topic.sessions ?? [];
  const chosenTab = entry && navigation.tabs[entry.topic.id];
  const shownTabId = shownTab(
    sessions.map((s) => s.id),
    chosenTab,
    entry ? (seenTabs.current[entry.topic.id] ?? []) : [],
  );
  const session = sessions.find((s) => s.id === shownTabId);
  const shownKey = JSON.stringify([view, session?.id]);

  useEffect(() => {
    const focus = focusWhenShown.current;
    focusWhenShown.current = null;
    focus?.();
  }, [shownKey]);

  useEffect(() => saveNavigation(navigation), [navigation]);

  useEffect(() => {
    if (!entry) return;
    const before = seenTabs.current[entry.topic.id] ?? [];
    seenTabs.current[entry.topic.id] = sessions.map((s) => s.id);
    // A tab just started here is chosen before the daemon's news of it has arrived.
    if (chosenTab === composeIn && chosenTab && !before.includes(chosenTab)) return;
    // Anything else is fixed in place, so another client's new tab doesn't take over.
    if (!shownTabId || shownTabId === chosenTab) return;
    setNavigation((current) => chooseTab(current, entry.topic.id, shownTabId));
    // Another client closed the tab this one was in, which took the keyboard with it.
    if (chosenTab && document.activeElement === document.body) focusPane();
  });

  // Requests on an offline device can't be answered, so they wait until it's back.
  const requests: RequestItem[] = devices.filter(isOnline).flatMap((device) =>
    (device.state?.approvals ?? []).map((approval) => ({
      approval,
      machineId: device.state!.machine.id,
      entry: topics.find(
        (t) =>
          t.machineId === device.state!.machine.id &&
          t.topic.sessions.some((s) => s.id === approval.session_id),
      ),
    })),
  );

  const approvals = requests
    .filter((item) => item.machineId === entry?.machineId && item.approval.session_id === session?.id)
    .map((item) => item.approval);

  const reportFailure = (reason: unknown) => setError(errorText(reason));

  const show = (next: View | null) => {
    const workspaceId =
      next?.kind === "topic"
        ? topics.find((t) => t.machineId === next.machineId && t.topic.id === next.topicId)
            ?.workspace.id
        : undefined;
    setNavigation((current) => goTo(current, next, workspaceId));
  };

  /** Runs `focus` now if nothing on screen is changing, or else once the change arrives. */
  const focusOnceShown = (changing: boolean, focus: () => void) => {
    if (changing) focusWhenShown.current = focus;
    else focus();
  };

  const isShown = (other: View | null) => JSON.stringify(other) === JSON.stringify(view);

  /** Shows a view while the keyboard stays where it is. */
  const preview = (next: View | null) => {
    setComposeIn(null);
    setError(null);
    show(next);
  };

  const open = (next: View | null) => {
    preview(next);
    focusOnceShown(!isShown(next), focusPane);
  };

  const selectTab = (topicId: string, sessionId: string) => {
    setComposeIn(null);
    setNavigation((current) => chooseTab(current, topicId, sessionId));
    focusOnceShown(sessionId !== session?.id, focusPane);
  };

  const openRequest = (item: RequestItem) => {
    if (!item.entry) return;
    preview({ kind: "topic", machineId: item.machineId, topicId: item.entry.topic.id });
    selectTab(item.entry.topic.id, item.approval.session_id);
  };

  const onCreated = (output: CommandOutput) => {
    if (output.output !== "session") return;
    setComposeIn(output.session.id);
    setNavigation((current) => chooseTab(current, output.topic_id, output.session.id));
  };

  const addWorkspaceOn = (device: Device) => {
    if (!device.state) return;
    const { id, name } = device.state.machine;
    setSheet({ kind: "workspace", machineId: id, deviceName: name, local: isLoopback(device.url) });
  };

  const openSettings = (machineId: string) => {
    preview({ kind: "device_settings", machineId });
  };

  const openAppSettings = (section = navigation.settingsSection) => {
    preview({ kind: "app_settings", section });
    // The keys here would otherwise reach whichever row or text box has focus.
    if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
  };

  /** The sidebar is back, and that's where settings are usually opened from. */
  const closeSettings = () => {
    const back = navigation.beforeSettings;
    preview(back);
    const focus = back?.kind === "new_topic" ? focusPane : focusSidebar;
    focusOnceShown(!isShown(back), () => requestAnimationFrame(focus));
  };

  const createTopicIn = (device: Device, workspace: Workspace) => {
    if (!device.state) return;
    open({ kind: "new_topic", machineId: device.state.machine.id, workspaceId: workspace.id });
  };

  // Where the new topic and new workspace keys create things. The palette and the workspace
  // select reach everywhere else.
  const onlineEntry = entry && isOnline(entry.device) ? entry : undefined;
  const workspaceDevice = onlineEntry?.device ?? devices.find(isOnline);
  const topicDevice =
    onlineEntry?.device ?? devices.find((d) => isOnline(d) && d.state?.workspaces.length);
  const topicWorkspace = onlineEntry?.workspace ?? topicDevice?.state?.workspaces[0];

  const newWorkspace = () => {
    if (workspaceDevice) addWorkspaceOn(workspaceDevice);
  };

  const newTopic = () => {
    if (view?.kind === "new_topic") focusPane();
    else if (topicDevice && topicWorkspace) createTopicIn(topicDevice, topicWorkspace);
  };

  const onTopicStarted = (machineId: string, topicId: string, sessionId: string) => {
    setNewTopicForm({});
    setNavigation((current) => chooseTab(current, topicId, sessionId));
    open({ kind: "topic", machineId, topicId });
  };

  /** The prompt stays in the form, because it never reached an agent. */
  const onTopicStalled = (machineId: string, topicId: string, reason: string) => {
    open({ kind: "topic", machineId, topicId });
    setError(`The topic was created, but its agent didn't start: ${reason}`);
  };

  const newAgent = () => {
    if (entry && isOnline(entry.device)) {
      setSheet({ kind: "agent", machineId: entry.machineId, topic: entry.topic });
    }
  };

  const interrupt = () => {
    if (entry && session && isActive(session)) {
      setError(null);
      request(entry.machineId, { command: "cancel_turn", session_id: session.id }).catch(
        reportFailure,
      );
    }
  };

  const moveTopic = (step: number) => {
    if (topics.length === 0) return;
    const index = topics.findIndex((t) => t === entry);
    const start = index === -1 && step < 0 ? 0 : index;
    const next = topics[(start + step + topics.length) % topics.length];
    open({ kind: "topic", machineId: next.machineId, topicId: next.topic.id });
  };

  const moveWorkspace = (step: number) => {
    const firsts = topics.filter(
      (t, index) => topics.findIndex((other) => other.workspace === t.workspace) === index,
    );
    const index = firsts.findIndex((t) => t.workspace === entry?.workspace);
    const first = firsts[(index + step + firsts.length) % firsts.length];
    const lastId = navigation.lastTopics[first.workspace.id];
    const next =
      topics.find((t) => t.workspace === first.workspace && t.topic.id === lastId) ?? first;
    open({ kind: "topic", machineId: next.machineId, topicId: next.topic.id });
  };

  const moveTab = (step: number) => {
    if (!entry || !session) return;
    const index = sessions.indexOf(session);
    const next = sessions[(index + step + sessions.length) % sessions.length];
    selectTab(entry.topic.id, next.id);
  };

  /** Stops the agent and removes its tab, asking first if it's still running. Without a tab
   * it does nothing. */
  const closeTab = () => {
    if (!entry || !session || !isOnline(entry.device)) return;
    if (isActive(session)) {
      setSheet({ kind: "close", machineId: entry.machineId, session });
      return;
    }
    setError(null);
    focusWhenShown.current = focusPane;
    request(entry.machineId, { command: "close_session", session_id: session.id }).catch(
      reportFailure,
    );
  };

  const renameTopic = (target: TopicEntry) => {
    if (!isOnline(target.device)) return;
    setSheet({ kind: "rename", machineId: target.machineId, topic: target.topic });
  };

  const deleteTopic = (target: TopicEntry) => {
    if (!isOnline(target.device)) return;
    setSheet({
      kind: "delete",
      machineId: target.machineId,
      workspace: target.workspace,
      topic: target.topic,
      fromSidebar: inSidebar(),
    });
  };

  /** The keyboard stays on its side. Deleting the shown topic moves on to one next to it. */
  const afterDeleting = (deleted: Extract<SheetRequest, { kind: "delete" }>) => {
    const { machineId, workspace, topic, fromSidebar } = deleted;
    const stayOnSide = fromSidebar ? () => requestAnimationFrame(focusSidebar) : focusPane;
    if (!isShown({ kind: "topic", machineId, topicId: topic.id })) {
      stayOnSide();
      return;
    }
    const next = landingAfterDeleting(machineId, workspace, topic.id);
    preview(next);
    focusWhenShown.current = next.kind === "new_topic" ? focusPane : stayOnSide;
  };

  const nextRequest = () => {
    const index = requests.findIndex(
      (item) => item.approval.session_id === session?.id && item.entry === entry,
    );
    const next = requests[(index + 1) % requests.length];
    if (next) openRequest(next);
  };

  const toggleSidebar = () => (inSidebar() ? focusPane() : focusSidebar());

  /** What's most useful here comes first, then every topic, then creating anywhere. */
  const paletteItems = (): PaletteItem[] => {
    const items: PaletteItem[] = [];
    if (entry && isOnline(entry.device)) {
      items.push({
        id: "agent",
        label: `New agent in ${entry.topic.name}`,
        keys: keysLabel("global.new_agent"),
        run: newAgent,
      });
      items.push({
        id: "rename-topic",
        label: `Rename ${entry.topic.name}`,
        detail: "Topic",
        keys: keysLabel("sidebar.rename"),
        run: () => renameTopic(entry),
      });
      items.push({
        id: "delete-topic",
        label: `Delete ${entry.topic.name}`,
        detail: "Topic",
        keys: keysLabel("sidebar.delete"),
        run: () => deleteTopic(entry),
      });
    }
    if (session && isActive(session)) {
      items.push({
        id: "interrupt",
        label: "Interrupt the agent",
        keys: keysLabel("global.interrupt"),
        run: () => {
          interrupt();
          focusPane();
        },
      });
    }
    if (session) {
      items.push({
        id: "toggle-steps",
        label: showsSteps() ? "Fold tool calls" : "Show tool calls",
        keys: keysLabel("pane.toggle_steps"),
        run: () => {
          toggleSteps().catch(reportFailure);
          focusPane();
        },
      });
    }
    if (entry && sessions.length > 1) {
      sessions.forEach((s, index) => {
        items.push({
          id: `tab/${s.id}`,
          label: sessionLabel(s),
          detail: "Tab",
          keys: index < 9 ? keysLabel(`global.tab_${index + 1}` as ActionId) : undefined,
          state: sessionState(s, isOnline(entry.device)),
          run: () => selectTab(entry.topic.id, s.id),
        });
      });
    }
    if (entry && session) {
      items.push({
        id: "close-tab",
        label: `Close ${sessionLabel(session)}`,
        detail: "Tab",
        keys: keysLabel("global.close_tab"),
        run: closeTab,
      });
    }
    if (requests.length) {
      items.push({
        id: "next",
        label: "Next request",
        keys: keysLabel("global.next_request"),
        run: nextRequest,
      });
    }
    items.push({
      id: "sidebar",
      label: "Go to the sidebar",
      keys: keysLabel("global.toggle_sidebar"),
      run: () => requestAnimationFrame(focusSidebar),
    });
    items.push({
      id: "app-settings",
      label: "Settings",
      keys: keysLabel("global.settings"),
      run: () => openAppSettings(),
    });
    items.push({
      id: "appearance",
      label: "Appearance",
      detail: "Theme, fonts and layout",
      run: () => openAppSettings("appearance"),
    });
    for (const t of topics) {
      items.push({
        id: `topic/${t.machineId}/${t.topic.id}`,
        label: t.topic.name,
        detail: `${t.workspace.name}, ${t.device.state?.machine.name}`,
        state: topicState(t.topic, isOnline(t.device)),
        run: () => open({ kind: "topic", machineId: t.machineId, topicId: t.topic.id }),
      });
    }
    for (const device of devices.filter(isOnline)) {
      const deviceName = device.state?.machine.name;
      if (device.state) {
        const { id } = device.state.machine;
        items.push({
          id: `settings/${device.url}`,
          label: `Settings for ${deviceName}`,
          run: () => openSettings(id),
        });
      }
      for (const workspace of device.state?.workspaces ?? []) {
        items.push({
          id: `new-topic/${workspace.id}`,
          label: `New topic in ${workspace.name}`,
          detail: deviceName,
          keys: workspace === topicWorkspace ? keysLabel("global.new_topic") : undefined,
          run: () => createTopicIn(device, workspace),
        });
      }
      items.push({
        id: `new-workspace/${device.url}`,
        label: `Add a workspace on ${deviceName}`,
        keys: device === workspaceDevice ? keysLabel("global.new_workspace") : undefined,
        run: () => addWorkspaceOn(device),
      });
    }
    return items;
  };

  const inSettings = view?.kind === "app_settings" || settingsDevice !== undefined;
  useHints(
    "global",
    globalHints({
      interrupt: session !== undefined && isActive(session),
      tabs: sessions.length,
      closeTab: entry !== undefined && session !== undefined && isOnline(entry.device),
      newAgent: entry !== undefined && isOnline(entry.device),
      newTopic: view?.kind !== "new_topic" && topicDevice !== undefined && topicWorkspace !== undefined,
      newWorkspace: workspaceDevice !== undefined,
      topics: topics.length,
      workspaces: new Set(topics.map((t) => t.workspace)).size,
      sidebar: !inSettings,
      settings: view?.kind !== "app_settings",
    }),
  );
  useHints("pinned", pinnedHints(requests.length));

  // Registered on every render, so the handlers always see the current state.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (sheet) return; // A sheet owns the keyboard.
      if (act(event, "global.palette", () => setPalette((shown) => !shown))) return;
      if (palette || overlayOpen()) return; // So does a palette or dropdown, past that key.

      const moveInWorkspaces = (step: number) => () => {
        if (entry) moveWorkspace(step);
        else moveTopic(step);
      };
      const commands: [ActionId, () => void][] = [
        ["global.prev_topic", () => moveTopic(-1)],
        ["global.next_topic", () => moveTopic(1)],
        ["global.prev_workspace", moveInWorkspaces(-1)],
        ["global.next_workspace", moveInWorkspaces(1)],
        ["global.settings", () => openAppSettings()],
        ["global.toggle_sidebar", toggleSidebar],
        ["global.new_workspace", newWorkspace],
        ["global.new_topic", newTopic],
        ["global.new_agent", newAgent],
        ["global.next_request", nextRequest],
        ["global.interrupt", interrupt],
        ["global.close_tab", closeTab],
        ["global.prev_tab", () => moveTab(-1)],
        ["global.next_tab", () => moveTab(1)],
      ];
      for (let tab = 1; tab <= 9; tab++) {
        commands.push([
          `global.tab_${tab}` as ActionId,
          () => {
            const target = sessions[tab - 1];
            if (entry && target) selectTab(entry.topic.id, target.id);
          },
        ]);
      }
      if (commands.some(([id, run]) => act(event, id, run))) return;

      const active = document.activeElement;
      const row = pointedRow() ?? (inSidebar() ? (active as HTMLElement) : null);
      if (row && act(event, "sidebar.add", () => pressAdd(row))) return;
      const rowTopic =
        row &&
        topics.find((t) => t.machineId === row.dataset.machine && t.topic.id === row.dataset.topic);
      if (rowTopic && act(event, "sidebar.rename", () => renameTopic(rowTopic))) return;
      if (rowTopic && act(event, "sidebar.delete", () => deleteTopic(rowTopic))) return;
      if (inPane(event)) {
        if (act(event, "pane.next_request", nextRequest)) return;
        // Settings decides for itself, because going back there depends on where you are in it.
        if (view?.kind === "device_settings") act(event, "device.back", closeSettings);
        else if (view?.kind !== "app_settings") act(event, "pane.back", focusSidebar);
      }
    };
    // Capturing sees the key before a sheet closes on it, so closing can't also leave the pane.
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  });

  return (
    <div className="app">
      {!settingsDevice && view?.kind !== "app_settings" && (
        <Sidebar
          devices={devices}
          view={view}
          onView={open}
          onPreview={preview}
          onSettings={openSettings}
          onAppSettings={() => openAppSettings()}
          onAddWorkspace={addWorkspaceOn}
          onNewTopic={createTopicIn}
        />
      )}
      {view?.kind === "app_settings" ? (
        <AppSettings
          section={view.section}
          onSection={(section) => show({ kind: "app_settings", section })}
          onClose={closeSettings}
        />
      ) : settingsDevice ? (
        <DeviceSettings device={settingsDevice} onClose={closeSettings} />
      ) : (
        <main className="main">
          {view?.kind === "new_topic" ? (
            <NewTopic
              devices={devices}
              machineId={view.machineId}
              workspaceId={view.workspaceId}
              form={newTopicForm}
              onForm={setNewTopicForm}
              onWorkspace={(machineId, workspaceId) =>
                preview({ kind: "new_topic", machineId, workspaceId })
              }
              onSettings={openSettings}
              onCancel={() => {
                setNewTopicForm({});
                open(navigation.beforeNewTopic);
              }}
              onStarted={onTopicStarted}
              onStalled={onTopicStalled}
            />
          ) : entry ? (
            <>
              <div className="tabs" role="tablist">
                {sessions.map((s, index) => (
                  <button
                    key={s.id}
                    role="tab"
                    aria-selected={s === session}
                    className={`tab ${s === session ? "shown" : ""}`}
                    title={index < 9 ? keysLabel(`global.tab_${index + 1}` as ActionId) : undefined}
                    onClick={() => selectTab(entry.topic.id, s.id)}
                  >
                    <StatusIcon state={sessionState(s, isOnline(entry.device))} />
                    {sessionLabel(s)}
                  </button>
                ))}
                <button
                  className="add"
                  disabled={!isOnline(entry.device)}
                  title={titled("New agent", "global.new_agent")}
                  aria-label="New agent"
                  onClick={newAgent}
                >
                  +
                </button>
              </div>
              {error && <p className="error main-error">{error}</p>}
              {session ? (
                <AgentPane
                  key={session.id}
                  machineId={entry.machineId}
                  session={session}
                  provider={entry.device.state?.providers?.find((p) => p.agent === session.agent)}
                  online={isOnline(entry.device)}
                  approvals={approvals}
                  draft={entry.device.state?.drafts?.[session.id] ?? ""}
                  compose={composeIn === session.id}
                  worktree={sessionWorktree(entry.topic, session)}
                  onInterrupt={isActive(session) ? interrupt : undefined}
                />
              ) : (
                <Empty>
                  <p>No agents in this topic yet</p>
                  <p className="empty-keys">
                    <Keys action="global.new_agent" /> Start an agent
                  </p>
                </Empty>
              )}
            </>
          ) : (
            <Welcome devices={devices} topics={topics} />
          )}
        </main>
      )}
      {sheet && (
        <Sheets
          sheet={sheet}
          workspaces={
            devices.find((d) => d.state?.machine.id === sheet.machineId)?.state?.workspaces ?? []
          }
          onClose={() => setSheet(null)}
          onDone={(output) => {
            onCreated(output);
            if (output.output === "workspace") {
              open(landingIn(navigation, sheet.machineId, output.workspace));
            }
            // The tab can already be gone by now, so this can't wait for the change.
            if (sheet.kind === "close") focusPane();
            if (sheet.kind === "delete") afterDeleting(sheet);
          }}
        />
      )}
      {palette && <Palette items={paletteItems()} onClose={() => setPalette(false)} />}
      {shownValue("layout.show_shortcuts") === true && <ShortcutBar />}
    </div>
  );
}

function Empty({ children }: { children: React.ReactNode }) {
  return <div className="pane empty">{children}</div>;
}

function Welcome({ devices, topics }: { devices: Device[]; topics: TopicEntry[] }) {
  if (!devices.some(isOnline)) {
    const failure = devices
      .map((device) => (device.connection.state === "offline" ? device.connection.reason : null))
      .find(Boolean);
    return (
      <Empty>
        <p>No device is reachable. Start a daemon on this machine:</p>
        <pre className="code empty-code">cargo run -p daemon</pre>
        {failure && <p className="empty-keys">{failure}</p>}
      </Empty>
    );
  }
  if (topics.length === 0) {
    return (
      <Empty>
        <p>Add a workspace, then create a topic in it</p>
        <p className="empty-keys">
          <Keys action="global.new_workspace" /> Add a workspace
        </p>
      </Empty>
    );
  }
  return (
    <Empty>
      <p>Choose a topic</p>
      <p className="empty-keys">
        <Keys action="global.palette" /> Find a topic
      </p>
    </Empty>
  );
}
