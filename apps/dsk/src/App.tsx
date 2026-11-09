import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { AgentPane } from "./AgentPane";
import { shownValue } from "./appearance";
import { AppSettings } from "./AppSettings";
import { DeviceSettings } from "./DeviceSettings";
import {
  type Approval,
  type Device,
  errorText,
  request,
  type Session,
  type Topic,
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
  toggleVim,
  useSettings,
  vimOn,
} from "./keybinds";
import {
  focusPane,
  focusSidebar,
  inPane,
  inSidebar,
  overlayOpen,
  pointedRow,
  pressAdd,
  sheetOpen,
  subscribeKeyboardPlace,
} from "./keys";
import {
  allTopics,
  isOnline,
  keepsAwake,
  type MoveChoice,
  moveChoices,
  sessionLabel,
  sessionState,
  type TopicEntry,
  topicState,
  topicWorktree,
  unreadToggle,
  withKeepAwakeToggled,
} from "./model";
import {
  chooseTab,
  closeNewTab,
  goTo,
  landingAfterArchiving,
  landingIn,
  loadNavigation,
  type Navigation,
  NEW_TAB,
  openNewTab,
  saveNavigation,
  shownTab,
  topicTabs,
  type View,
} from "./navigation";
import { NEW_TAB_STATE, NewTab, type NewTabState } from "./NewTab";
import { NewTopic, type NewTopicForm } from "./NewTopic";
import { Palette, type PaletteItem, screenPaletteItems } from "./Palette";
import { isLoopback } from "./pathInput";
import { Sheets, type SheetRequest } from "./Sheets";
import { globalHints, pinnedHints, useHints } from "./shortcuts";
import { Sidebar } from "./Sidebar";
import { TopicMcpSheet } from "./TopicMcp";
import { Icon } from "./ui/Icon";
import { ShortcutBar } from "./ui/ShortcutBar";
import { Keys, StatusIcon } from "./ui/StatusIcon";

const TAB_ACTIONS: ActionId[] = [
  "global.tab_1",
  "global.tab_2",
  "global.tab_3",
  "global.tab_4",
  "global.tab_5",
  "global.tab_6",
  "global.tab_7",
  "global.tab_8",
  "global.tab_9",
];

/** A list to choose from in the palette's place, like archived topics. */
interface Picker {
  placeholder: string;
  empty: string;
  items: PaletteItem[];
}

/** A topic on its way out, and where the keyboard was when it left. */
interface Leaving {
  machineId: string;
  workspace: Workspace;
  topic: Topic;
  fromSidebar: boolean;
}

const archivedOn = (ms: number) =>
  new Date(ms).toLocaleDateString(undefined, { month: "short", day: "numeric" });

interface RequestItem {
  approval: Approval;
  machineId: string;
  entry: TopicEntry | undefined;
}

/** How long a finished tab stays unread while you look at it, so you see it land. */
const READ_AFTER_MS = 1000;

/** Whether this window is the one in front. Nothing gets read while it isn't. */
function useWindowFocused(): boolean {
  const [focused, setFocused] = useState(() => document.hasFocus());
  useEffect(() => {
    const update = () => setFocused(document.hasFocus());
    window.addEventListener("focus", update);
    window.addEventListener("blur", update);
    return () => {
      window.removeEventListener("focus", update);
      window.removeEventListener("blur", update);
    };
  }, []);
  return focused;
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
  // Each topic's new tab, kept while you look elsewhere. Topic ID to state.
  const [newTabStates, setNewTabStates] = useState<Record<string, NewTabState>>({});
  const [sheet, setSheet] = useState<SheetRequest | null>(null);
  const [palette, setPalette] = useState(false);
  const [picker, setPicker] = useState<Picker | null>(null);
  // The topic whose MCP servers are being chosen. It reads the topic live, so changes show.
  const [mcpTopic, setMcpTopic] = useState<{ machineId: string; topicId: string } | null>(null);
  // The tab just started here, which is chosen before the daemon's news of it arrives.
  const [startedHere, setStartedHere] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Where the keyboard goes once the screen it was waiting for has rendered.
  const focusWhenShown = useRef<(() => void) | null>(null);
  // The tabs each topic last showed, which says where to go when the shown one is closed.
  const seenTabs = useRef<Record<string, string[]>>({});
  // Tabs marked unread here, which looking at them doesn't read until you've looked away.
  const [keptUnread, setKeptUnread] = useState<string[]>([]);
  const focused = useWindowFocused();
  const onSidebar = useSyncExternalStore(subscribeKeyboardPlace, inSidebar);

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
  const tabIds = entry
    ? topicTabs(
        navigation,
        entry.topic.id,
        sessions.map((s) => s.id),
      )
    : [];
  const chosenTab = entry && navigation.tabs[entry.topic.id];
  const shownTabId = shownTab(
    tabIds,
    chosenTab,
    entry ? (seenTabs.current[entry.topic.id] ?? []) : [],
  );
  const session = sessions.find((s) => s.id === shownTabId);
  const onNewTab = entry !== undefined && shownTabId === NEW_TAB;
  const shownKey = JSON.stringify([view, shownTabId]);

  useEffect(() => {
    const focus = focusWhenShown.current;
    focusWhenShown.current = null;
    focus?.();
  }, [shownKey]);

  useEffect(() => saveNavigation(navigation), [navigation]);

  useEffect(() => setKeptUnread([]), [shownKey]);

  // Being in a finished tab reads it, on every client. Passing over its topic in the sidebar
  // shows it too, but doesn't count: it stays green until the keyboard is in the pane.
  const readable =
    entry &&
    session?.unread &&
    focused &&
    !onSidebar &&
    isOnline(entry.device) &&
    !keptUnread.includes(session.id);
  const readMachine = readable ? entry.machineId : null;
  const readSession = readMachine ? session?.id : undefined;
  useEffect(() => {
    if (!readMachine || !readSession) return;
    const read = () =>
      request(readMachine, { command: "set_unread", session_id: readSession, unread: false })
        // Nothing to tell anyone: it's tried again the next time the tab is shown.
        .catch(() => {});
    const timer = setTimeout(read, READ_AFTER_MS);
    return () => clearTimeout(timer);
  }, [readMachine, readSession]);

  useEffect(() => {
    if (!entry) return;
    seenTabs.current[entry.topic.id] = tabIds;
    // A tab just started here is chosen before the daemon's news of it has arrived.
    if (chosenTab === startedHere && chosenTab && !tabIds.includes(chosenTab)) return;
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
    setStartedHere(null);
    setError(null);
    show(next);
  };

  const open = (next: View | null) => {
    preview(next);
    focusOnceShown(!isShown(next), focusPane);
  };

  const selectTab = (topicId: string, tabId: string) => {
    setStartedHere(null);
    setNavigation((current) => chooseTab(current, topicId, tabId));
    focusOnceShown(tabId !== shownTabId, focusPane);
  };

  const openRequest = (item: RequestItem) => {
    if (!item.entry) return;
    preview({ kind: "topic", machineId: item.machineId, topicId: item.entry.topic.id });
    selectTab(item.entry.topic.id, item.approval.session_id);
  };

  const addWorkspaceOn = (device: Device) => {
    if (!device.state) return;
    const { id, name } = device.state.machine;
    setSheet({ kind: "workspace", machineId: id, deviceName: name, local: isLoopback(device.url) });
  };

  const renameDevice = (device: Device) => {
    if (!device.state || !isOnline(device)) return;
    const { id, name } = device.state.machine;
    setSheet({ kind: "rename_device", machineId: id, name });
  };

  const openSettings = (machineId: string, section?: string) => {
    preview({ kind: "device_settings", machineId, section });
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

  const newTab = () => {
    if (!entry || !isOnline(entry.device)) return;
    const topicId = entry.topic.id;
    setStartedHere(null);
    setNavigation((current) => openNewTab(current, topicId));
    focusOnceShown(!onNewTab, focusPane);
  };

  const setNewTabState = (topicId: string, state: NewTabState) => {
    setNewTabStates((current) => ({ ...current, [topicId]: state }));
    // Choosing in a topic's only tab makes it one you opened, so an agent another client
    // starts there doesn't take it away mid-sentence.
    if (state.kind) {
      setNavigation((current) =>
        current.newTabs.includes(topicId) ? current : openNewTab(current, topicId),
      );
    }
  };

  const forgetNewTab = (topicId: string) =>
    setNewTabStates((current) => {
      const next = { ...current };
      delete next[topicId];
      return next;
    });

  /** The new tab gives way to the agent started from it. */
  const replaceNewTab = (topicId: string, sessionId: string) => {
    setStartedHere(sessionId);
    setNavigation((current) => chooseTab(closeNewTab(current, topicId), topicId, sessionId));
    focusOnceShown(true, focusPane);
  };

  const onTabStarted = (topicId: string, sessionId: string) => {
    forgetNewTab(topicId);
    replaceNewTab(topicId, sessionId);
  };

  /** The prompt stays in the new tab, because it never reached the agent. */
  const onTabStalled = (topicId: string, sessionId: string, reason: string) => {
    replaceNewTab(topicId, sessionId);
    setError(`The agent started, but its prompt didn't reach it: ${reason}`);
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
    if (!entry || !shownTabId) return;
    const index = tabIds.indexOf(shownTabId);
    const next = tabIds[(index + step + tabIds.length) % tabIds.length];
    selectTab(entry.topic.id, next);
  };

  // A topic's only tab is its new tab, which stays because there's nothing else to show.
  const canCloseNewTab = onNewTab && sessions.length > 0;
  const canClose =
    canCloseNewTab || (session !== undefined && entry !== undefined && isOnline(entry.device));

  /** Stops the agent and removes its tab, asking first if it's still running. A new tab
   * closes with what was typed in it. */
  const closeTab = () => {
    if (entry && canCloseNewTab) {
      const topicId = entry.topic.id;
      forgetNewTab(topicId);
      setNavigation((current) => closeNewTab(current, topicId));
      focusOnceShown(true, focusPane);
      return;
    }
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

  const setUnread = (machineId: string, changed: Session[], unread: boolean) => {
    for (const s of changed) {
      request(machineId, { command: "set_unread", session_id: s.id, unread }).catch(reportFailure);
    }
  };

  const toggleUnread = (target: TopicEntry) => {
    if (!isOnline(target.device)) return;
    const { unread, sessions: changed } = unreadToggle(target.topic);
    if (unread) setKeptUnread(changed.map((s) => s.id));
    setUnread(target.machineId, changed, unread);
  };

  const unreadTopics = topics.filter(
    (t) => isOnline(t.device) && t.topic.sessions.some((s) => s.unread),
  );
  const markAllRead = () => {
    for (const t of unreadTopics) {
      setUnread(t.machineId, t.topic.sessions.filter((s) => s.unread), false);
    }
  };

  const openTopicMcp = () => {
    if (entry?.device.state) setMcpTopic({ machineId: entry.machineId, topicId: entry.topic.id });
  };
  const mcpEntry =
    mcpTopic &&
    topics.find((t) => t.machineId === mcpTopic.machineId && t.topic.id === mcpTopic.topicId);
  const mcpGone = mcpTopic !== null && !mcpEntry?.device.state;
  // Another client can delete the topic, which takes the sheet with it.
  useEffect(() => {
    if (mcpGone) setMcpTopic(null);
  }, [mcpGone]);

  const renameTopic = (target: TopicEntry) => {
    if (!isOnline(target.device)) return;
    setSheet({ kind: "rename", machineId: target.machineId, topic: target.topic });
  };

  /** Archives straight away when that loses nothing, and asks first when it would. */
  const archiveTopic = (target: TopicEntry) => {
    if (!isOnline(target.device)) return;
    const leaving: Leaving = {
      machineId: target.machineId,
      workspace: target.workspace,
      topic: target.topic,
      fromSidebar: inSidebar(),
    };
    setError(null);
    request(target.machineId, { command: "archive_topic", topic_id: target.topic.id })
      .then((output) => {
        if (output.output === "archive_risk") {
          setSheet({ kind: "archive", ...leaving, risk: output.risk });
        } else {
          afterArchiving(leaving);
        }
      })
      .catch(reportFailure);
  };

  /** The keyboard stays on its side. Archiving the shown topic moves on to one next to it. */
  const afterArchiving = (leaving: Leaving) => {
    const { machineId, workspace, topic, fromSidebar } = leaving;
    const stayOnSide = fromSidebar ? () => requestAnimationFrame(focusSidebar) : focusPane;
    if (!isShown({ kind: "topic", machineId, topicId: topic.id })) {
      stayOnSide();
      return;
    }
    const next = landingAfterArchiving(machineId, workspace, topic.id);
    preview(next);
    focusWhenShown.current = next.kind === "new_topic" ? focusPane : stayOnSide;
  };

  const restoreTopic = (machineId: string, topicId: string) => {
    setError(null);
    request(machineId, { command: "restore_topic", topic_id: topicId })
      .then(() => open({ kind: "topic", machineId, topicId }))
      .catch(reportFailure);
  };

  /** Archived topics aren't in any device's state, so each device is asked when this opens. */
  const findArchived = async () => {
    const lists = await Promise.all(
      devices.filter(isOnline).map(async (device) => {
        const machineId = device.state!.machine.id;
        const output = await request(machineId, { command: "list_archived_topics" });
        const archived = output.output === "archived_topics" ? output.topics : [];
        return archived.map((topic) => ({ device, machineId, topic }));
      }),
    );
    const items = lists
      .flat()
      .sort((a, b) => b.topic.archived_at_ms - a.topic.archived_at_ms)
      .map(({ device, machineId, topic }) => ({
        id: `archived/${machineId}/${topic.id}`,
        label: topic.name,
        detail: [
          device.state?.workspaces.find((w) => w.id === topic.workspace_id)?.name,
          device.state?.machine.name,
          archivedOn(topic.archived_at_ms),
        ]
          .filter(Boolean)
          .join(", "),
        run: () => restoreTopic(machineId, topic.id),
      }));
    setPicker({ placeholder: "Restore an archived topic", empty: "No archived topics", items });
  };

  const moveToFolder = (target: TopicEntry, choice: MoveChoice) => {
    if (!isOnline(target.device)) return;
    if (target.topic.sessions.some(isActive)) {
      setSheet({ kind: "move", machineId: target.machineId, topic: target.topic, choice });
      return;
    }
    setError(null);
    request(target.machineId, {
      command: "move_topic",
      topic_id: target.topic.id,
      to: choice.target,
    }).catch(reportFailure);
  };

  const pickFolder = (target: TopicEntry) => {
    if (!isOnline(target.device)) return;
    const items = moveChoices(target.topic, target.workspace.path).map((choice) => ({
      id: `move/${choice.id}`,
      label: choice.label,
      detail: choice.detail,
      run: () => moveToFolder(target, choice),
    }));
    setPicker({ placeholder: `Move ${target.topic.name} to`, empty: "Nowhere to move", items });
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
        id: "new-tab",
        label: `New tab in ${entry.topic.name}`,
        keys: keysLabel("global.new_tab"),
        run: newTab,
      });
      items.push({
        id: "topic-mcp",
        label: `MCP servers for ${entry.topic.name}`,
        detail: "Topic",
        keys: keysLabel("global.topic_mcp"),
        run: openTopicMcp,
      });
      items.push({
        id: "rename-topic",
        label: `Rename ${entry.topic.name}`,
        detail: "Topic",
        keys: keysLabel("sidebar.rename"),
        run: () => renameTopic(entry),
      });
      const toggle = unreadToggle(entry.topic);
      if (toggle.sessions.length) {
        items.push({
          id: "toggle-unread",
          label: `Mark ${entry.topic.name} ${toggle.unread ? "unread" : "read"}`,
          detail: "Topic",
          keys: keysLabel("sidebar.toggle_unread"),
          run: () => toggleUnread(entry),
        });
      }
      items.push({
        id: "move-topic",
        label: `Move ${entry.topic.name} to another folder`,
        detail: topicWorktree(entry.topic)?.branch,
        keys: keysLabel("global.move_topic"),
        run: () => pickFolder(entry),
      });
      items.push({
        id: "archive-topic",
        label: `Archive ${entry.topic.name}`,
        detail: "Topic",
        keys: keysLabel("sidebar.archive"),
        run: () => archiveTopic(entry),
      });
    }
    items.push(...screenPaletteItems());
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
    items.push({
      id: "toggle-vim",
      label: vimOn() ? "Turn vim keys off" : "Turn vim keys on",
      detail: "Message boxes",
      keys: keysLabel("global.toggle_vim"),
      run: () => {
        toggleVim().catch(reportFailure);
        focusPane();
      },
    });
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
    if (entry && tabIds.length > 1) {
      tabIds.forEach((id, index) => {
        const s = sessions.find((other) => other.id === id);
        items.push({
          id: `tab/${id}`,
          label: s ? sessionLabel(s) : "New tab",
          detail: "Tab",
          keys: index < 9 ? keysLabel(TAB_ACTIONS[index]) : undefined,
          state: s && sessionState(s, isOnline(entry.device)),
          run: () => selectTab(entry.topic.id, id),
        });
      });
    }
    if (entry && canClose) {
      items.push({
        id: "close-tab",
        label: `Close ${session ? sessionLabel(session) : "the new tab"}`,
        detail: "Tab",
        keys: keysLabel("global.close_tab"),
        run: closeTab,
      });
    }
    if (unreadTopics.length) {
      items.push({
        id: "mark-all-read",
        label: "Mark every topic read",
        keys: keysLabel("global.mark_all_read"),
        run: markAllRead,
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
    if (devices.some(isOnline)) {
      items.push({
        id: "archived-topics",
        label: "Restore an archived topic",
        keys: keysLabel("global.archived_topics"),
        run: () => void findArchived().catch(reportFailure),
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
        items.push({
          id: `mcp/${device.url}`,
          label: `MCP servers on ${deviceName}`,
          detail: "Device settings",
          run: () => openSettings(id, "mcp"),
        });
        items.push({
          id: `rename-device/${device.url}`,
          label: `Rename ${deviceName}`,
          keys: device === settingsDevice ? keysLabel("device.rename") : undefined,
          run: () => renameDevice(device),
        });
        const state = device.state;
        items.push({
          id: `keep-awake/${device.url}`,
          label: keepsAwake(state)
            ? `Let ${deviceName} sleep while agents work`
            : `Keep ${deviceName} awake while agents work`,
          keys: device === settingsDevice ? keysLabel("device.toggle_keep_awake") : undefined,
          run: () => {
            const settings = withKeepAwakeToggled(state);
            request(id, { command: "set_device_settings", settings }).catch(reportFailure);
          },
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
      tabs: tabIds.length,
      closeTab: canClose,
      newTab: entry !== undefined && isOnline(entry.device),
      topicMcp: entry !== undefined && !inSettings,
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
      // A sheet or a list to choose from owns the keyboard, and so does a screen's own sheet,
      // which only the palette's own key gets past to close the palette.
      if (sheet || picker || mcpTopic || (!palette && sheetOpen())) return;
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
        ["global.new_tab", newTab],
        ["global.next_request", nextRequest],
        ["global.interrupt", interrupt],
        ["global.close_tab", closeTab],
        ["global.prev_tab", () => moveTab(-1)],
        ["global.next_tab", () => moveTab(1)],
        ["global.mark_all_read", markAllRead],
        ["global.toggle_vim", () => toggleVim().catch(reportFailure)],
        ["global.move_topic", () => entry && pickFolder(entry)],
        ["global.archived_topics", () => void findArchived().catch(reportFailure)],
        ["global.topic_mcp", openTopicMcp],
      ];
      TAB_ACTIONS.forEach((id, index) => {
        commands.push([
          id,
          () => {
            const target = tabIds[index];
            if (entry && target) selectTab(entry.topic.id, target);
          },
        ]);
      });
      if (commands.some(([id, run]) => act(event, id, run))) return;

      const active = document.activeElement;
      const row = pointedRow() ?? (inSidebar() ? (active as HTMLElement) : null);
      if (row && act(event, "sidebar.add", () => pressAdd(row))) return;
      const rowTopic =
        row &&
        topics.find((t) => t.machineId === row.dataset.machine && t.topic.id === row.dataset.topic);
      if (rowTopic && act(event, "sidebar.rename", () => renameTopic(rowTopic))) return;
      if (rowTopic && act(event, "sidebar.archive", () => archiveTopic(rowTopic))) return;
      if (rowTopic && act(event, "sidebar.toggle_unread", () => toggleUnread(rowTopic))) return;
      if (inPane(event)) {
        if (act(event, "pane.next_request", nextRequest)) return;
        // Settings decides for itself, because going back there depends on where you are in it.
        if (view?.kind !== "device_settings" && view?.kind !== "app_settings") {
          act(event, "pane.back", focusSidebar);
        }
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
        <DeviceSettings
          device={settingsDevice}
          section={view?.kind === "device_settings" ? view.section : undefined}
          onSection={(section) => {
            if (view?.kind === "device_settings") show({ ...view, section });
          }}
          onRename={() => renameDevice(settingsDevice)}
          onClose={closeSettings}
        />
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
                {tabIds.map((id, index) => {
                  const s = sessions.find((other) => other.id === id);
                  return (
                    <button
                      key={id}
                      role="tab"
                      aria-selected={id === shownTabId}
                      className={`tab ${id === shownTabId ? "shown" : ""}`}
                      title={index < 9 ? keysLabel(TAB_ACTIONS[index]) : undefined}
                      onClick={() => selectTab(entry.topic.id, id)}
                    >
                      {s && <StatusIcon state={sessionState(s, isOnline(entry.device))} />}
                      <span className="tab-label">{s ? sessionLabel(s) : "New tab"}</span>
                    </button>
                  );
                })}
                <button
                  className="add"
                  disabled={!isOnline(entry.device)}
                  title={titled("New tab", "global.new_tab")}
                  aria-label="New tab"
                  onClick={newTab}
                >
                  <Icon name="add" />
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
                  worktree={topicWorktree(entry.topic)}
                  onInterrupt={isActive(session) ? interrupt : undefined}
                />
              ) : (
                <NewTab
                  key={entry.topic.id}
                  entry={entry}
                  state={newTabStates[entry.topic.id] ?? NEW_TAB_STATE}
                  onState={(state) => setNewTabState(entry.topic.id, state)}
                  onSettings={openSettings}
                  onStarted={(sessionId) => onTabStarted(entry.topic.id, sessionId)}
                  onStalled={(sessionId, reason) =>
                    onTabStalled(entry.topic.id, sessionId, reason)
                  }
                />
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
            if (output.output === "workspace") {
              open(landingIn(navigation, sheet.machineId, output.workspace));
            }
            // The tab can already be gone by now, so this can't wait for the change.
            if (sheet.kind === "close") focusPane();
            if (sheet.kind === "archive") afterArchiving(sheet);
          }}
        />
      )}
      {mcpEntry?.device.state && (
        <TopicMcpSheet
          state={mcpEntry.device.state}
          topic={mcpEntry.topic}
          online={isOnline(mcpEntry.device)}
          onClose={() => setMcpTopic(null)}
        />
      )}
      {palette && <Palette items={paletteItems()} onClose={() => setPalette(false)} />}
      {picker && (
        <Palette
          items={picker.items}
          placeholder={picker.placeholder}
          empty={picker.empty}
          onClose={() => setPicker(null)}
        />
      )}
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
