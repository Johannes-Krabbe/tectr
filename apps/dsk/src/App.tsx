import { lazy, Suspense, useEffect, useRef, useState, useSyncExternalStore } from "react";
import { AgentPane } from "./AgentPane";
import { AppSettings } from "./AppSettings";
import { DeviceSettings } from "./DeviceSettings";
import {
  type Approval,
  type Device,
  errorText,
  type PromptImage,
  forgetDevice,
  reconnect,
  request,
  type Session,
  type Topic,
  useDevices,
  useWaiting,
  type Workspace,
} from "./engine";
import {
  act,
  type ActionId,
  diffDefaultScope,
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
  subscribeFocus,
} from "./keys";
import {
  allTopics,
  connectionProblem,
  deviceNotice,
  isOnline,
  type MoveChoice,
  moveChoices,
  sessionState,
  tabLabel,
  tabsOf,
  type TopicEntry,
  topicWorktree,
  unreadToggle,
} from "./model";
import {
  awaitingStartedTab,
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
import { appItems, type Runnable, topicItems, withRuns } from "./menus";
import { DiffPane, forgetDiffView } from "./DiffPane";
import { NEW_TAB_STATE, NewTab, type NewTabState } from "./NewTab";
import { NewTopic, type NewTopicForm } from "./NewTopic";
import { isLoopback } from "./pathInput";
import { appSections, settingsGroups } from "./settingsNav";
import { Sheets, type SheetRequest } from "./Sheets";
import { Sidebar } from "./Sidebar";
import { Icon } from "./ui/Icon";
import { MenuButton, type MenuGroups } from "./ui/Menu";
import type { SettingsNav } from "./ui/SettingsPage";
import { Toasts } from "./ui/Toasts";
import { StatusIcon } from "./ui/StatusIcon";
import { WorkspacePage } from "./WorkspacePage";

/** One for every session without draft images, so a pane's images stay the same object. */
const NO_IMAGES: PromptImage[] = [];
// xterm.js is most of the app's code, so it loads when the first terminal is shown rather than
// with the app.
const loadTerminalPane = () => import("./TerminalPane");
const TerminalPane = lazy(() =>
  loadTerminalPane().then((module) => ({ default: module.TerminalPane })),
);
/** Loaded soon after starting, out of the way of the first paint, so a terminal opens at once. */
const PRELOAD_TERMINAL_MS = 2000;

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

/** A topic on its way out, and where the keyboard was when it left. */
interface Leaving {
  machineId: string;
  workspace: Workspace;
  topic: Topic;
  fromSidebar: boolean;
}

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
  const settings = useSettings();
  // What exists comes from the daemon, so every client sees the same. What this client is
  // looking at is its own, and the rest is about this keyboard and this moment.
  const [navigation, setNavigation] = useState<Navigation>(loadNavigation);
  const [newTopicForm, setNewTopicForm] = useState<NewTopicForm>({});
  // Each topic's new tab, kept while you look elsewhere. Topic ID to state.
  const [newTabStates, setNewTabStates] = useState<Record<string, NewTabState>>({});
  const [sheet, setSheet] = useState<SheetRequest | null>(null);
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
  const onSidebar = useSyncExternalStore(subscribeFocus, inSidebar);

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
  const pageDevice =
    view?.kind === "workspace"
      ? devices.find((d) => d.state?.machine.id === view.machineId)
      : undefined;
  const pageWorkspace =
    view?.kind === "workspace"
      ? pageDevice?.state?.workspaces.find((w) => w.id === view.workspaceId)
      : undefined;
  const sessions = entry?.topic.sessions ?? [];
  const tabs = entry ? tabsOf(entry.topic) : [];
  const tabIds = entry
    ? topicTabs(
        navigation,
        entry.topic.id,
        tabs.map((tab) => tab.id),
      )
    : [];
  const chosenTab = entry && navigation.tabs[entry.topic.id];
  const shownTabId = shownTab(
    tabIds,
    chosenTab,
    entry ? (seenTabs.current[entry.topic.id] ?? []) : [],
  );
  const session = sessions.find((s) => s.id === shownTabId);
  const shownTerminal = entry?.topic.terminals?.find((t) => t.id === shownTabId);
  const shownDiff = entry?.topic.diffs?.find((d) => d.id === shownTabId);
  const onNewTab = entry !== undefined && shownTabId === NEW_TAB;
  const shownKey = JSON.stringify([view, shownTabId]);

  useEffect(() => {
    if (awaitingStartedTab(startedHere, chosenTab, shownTabId)) return;
    const focus = focusWhenShown.current;
    focusWhenShown.current = null;
    focus?.();
  }, [shownKey]);

  useEffect(() => saveNavigation(navigation), [navigation]);

  useEffect(() => {
    const timer = setTimeout(() => void loadTerminalPane(), PRELOAD_TERMINAL_MS);
    return () => clearTimeout(timer);
  }, []);

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

  /** Tries the devices that are away now, rather than whenever their next attempt is due. */
  const reconnectNow = () => void reconnect().catch(reportFailure);

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

  // Pairing starts on the device itself, which is the one reached over loopback.
  const localDevice = devices.find((d) => isLoopback(d.url) && isOnline(d));

  const pairWith = async (device: Device) => {
    if (!device.state || !isOnline(device)) return;
    const { id, name } = device.state.machine;
    const output = await request(id, { command: "start_pairing" });
    if (output.output !== "pairing") return;
    setSheet({
      kind: "pairing",
      machineId: id,
      deviceName: name,
      code: output.code,
      expiresAtMs: output.expires_at_ms,
    });
  };

  const forget = async (device: Device) => {
    if (device === settingsDevice) closeSettings();
    await forgetDevice(device.url);
  };

  const openSettings = (machineId: string, section?: string) => {
    preview({ kind: "device_settings", machineId, section });
  };

  const openAppSettings = (section = navigation.settingsSection) => {
    preview({ kind: "app_settings", section });
    // Settings start with the keyboard on their list of sections, rather than wherever it was.
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

  /** Where the empty window's button adds a workspace: the first device that can take one. */
  const firstOnline = devices.find(isOnline);

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

  const onTabStarted = (topicId: string, tabId: string) => {
    forgetNewTab(topicId);
    replaceNewTab(topicId, tabId);
  };

  /** Starts a shell in the topic's folder. From the new tab it takes the new tab's place. The
   * size is a guess that the terminal corrects as soon as it's shown. */
  const openTerminal = () => {
    if (!entry || !isOnline(entry.device)) return;
    const { machineId, topic } = entry;
    const fromNewTab = onNewTab;
    setError(null);
    request(machineId, { command: "open_terminal", topic_id: topic.id, cols: 80, rows: 24 })
      .then((output) => {
        if (output.output !== "terminal") return;
        if (fromNewTab) {
          onTabStarted(topic.id, output.terminal.id);
        } else {
          setStartedHere(output.terminal.id);
          selectTab(topic.id, output.terminal.id);
        }
      })
      .catch(reportFailure);
  };

  /** Opens a diff of the topic's folder, which from the new tab takes the new tab's place. */
  const openDiff = () => {
    if (!entry || !isOnline(entry.device)) return;
    const { machineId, topic } = entry;
    const fromNewTab = onNewTab;
    setError(null);
    request(machineId, { command: "open_diff", topic_id: topic.id, scope: { kind: diffDefaultScope() } })
      .then((output) => {
        if (output.output !== "diff") return;
        if (fromNewTab) {
          onTabStarted(topic.id, output.diff.id);
        } else {
          setStartedHere(output.diff.id);
          selectTab(topic.id, output.diff.id);
        }
      })
      .catch(reportFailure);
  };

  /** Carries on a conversation from another agent's files as a tab, in the new tab's place. */
  const importConversation = (agent: string, id: string) => {
    if (!entry || !isOnline(entry.device)) return;
    const { machineId, topic } = entry;
    setError(null);
    request(machineId, { command: "import_conversation", topic_id: topic.id, agent, id })
      .then((imported) => {
        if (imported.output === "session") onTabStarted(topic.id, imported.session.id);
      })
      .catch(reportFailure);
  };

  /** Opens the topic's new tab on its import page, where the conversation is chosen. */
  const openImport = (target: TopicEntry) => {
    if (!isOnline(target.device)) return;
    const topicId = target.topic.id;
    const already = target === entry && onNewTab;
    setNewTabStates((current) => ({
      ...current,
      [topicId]: { ...(current[topicId] ?? NEW_TAB_STATE), kind: "import" },
    }));
    preview({ kind: "topic", machineId: target.machineId, topicId });
    setNavigation((current) => openNewTab(current, topicId));
    focusOnceShown(!already, focusPane);
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
  const canCloseTab = (id: string | undefined) =>
    entry !== undefined &&
    id !== undefined &&
    (id === NEW_TAB ? tabs.length > 0 : isOnline(entry.device));

  /** Stops the agent and removes its tab, asking first if it's still running. A new tab
   * closes with what was typed in it. It's the shown tab unless another is named, which its
   * own close button does. */
  const closeTab = (id = shownTabId) => {
    if (!entry || !id || !canCloseTab(id)) return;
    const { machineId, topic } = entry;
    // Only closing the shown tab shows another one, which the keyboard then goes to.
    const shown = id === shownTabId;
    if (id === NEW_TAB) {
      forgetNewTab(topic.id);
      setNavigation((current) => closeNewTab(current, topic.id));
      if (shown) focusOnceShown(true, focusPane);
      return;
    }
    const closing = sessions.find((s) => s.id === id);
    if (closing && isActive(closing)) {
      setSheet({ kind: "close", machineId, session: closing });
      return;
    }
    setError(null);
    if (shown) focusWhenShown.current = focusPane;
    const isDiff = topic.diffs?.some((d) => d.id === id);
    if (isDiff) forgetDiffView(id);
    request(
      machineId,
      closing
        ? { command: "close_session", session_id: id }
        : isDiff
          ? { command: "close_diff", diff_id: id }
          : // Its shell ends with it, as closing a terminal window does.
            { command: "close_terminal", terminal_id: id },
    ).catch(reportFailure);
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

  /** Its page has its topics and the MCP servers its agents get. */
  const openWorkspace = (target = entry) => {
    if (target) open({ kind: "workspace", machineId: target.machineId, workspaceId: target.workspace.id });
  };

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

  const nextRequest = () => {
    const index = requests.findIndex(
      (item) => item.approval.session_id === session?.id && item.entry === entry,
    );
    const next = requests[(index + 1) % requests.length];
    if (next) openRequest(next);
  };

  const toggleSidebar = () => (inSidebar() ? focusPane() : focusSidebar());

  /** What a topic offers, with what runs it. Nothing while its device can't be reached. */
  const topicActions = (target: TopicEntry): Runnable[][] => {
    if (!isOnline(target.device)) return [];
    const toggle = unreadToggle(target.topic);
    const groups = withRuns(
      topicItems({ toggle: { unread: toggle.unread, count: toggle.sessions.length } }),
      {
        "rename-topic": () => renameTopic(target),
        "toggle-unread": () => toggleUnread(target),
        "move-topic": () => {},
        "open-workspace": () => openWorkspace(target),
        "import-conversation": () => openImport(target),
        "archive-topic": () => archiveTopic(target),
      },
    );
    // There are only ever a few places to move to, so they're chosen right in the menu.
    const places = moveChoices(target.topic, target.workspace.path).map((choice) => ({
      id: `move/${choice.id}`,
      label: choice.label,
      run: () => moveToFolder(target, choice),
    }));
    return groups.map((group) =>
      group.map((item) => (item.id === "move-topic" ? { ...item, items: places } : item)),
    );
  };

  /** What's about every topic at once, with what runs it. */
  const appActions = (): Runnable[][] =>
    withRuns(
      appItems({ requests: requests.length, unread: unreadTopics.length }),
      { next: nextRequest, "mark-all-read": markAllRead },
    );

  const asMenu = (groups: Runnable[][]): MenuGroups =>
    groups.map((group) =>
      group.map(({ id, label, key, items, run }) => ({ id, label, keys: key && keysLabel(key), items, run })),
    );

  /** The tab bar's menu: what the topic offers, then how an agent's tab shows its work, which is
   * a setting and so works while the device is away. */
  const tabBarMenu = (target: TopicEntry): MenuGroups => {
    const groups = asMenu(topicActions(target));
    if (!session) return groups;
    const steps = {
      id: "toggle-steps",
      label: showsSteps() ? "Fold tool calls" : "Show tool calls",
      run: () => void toggleSteps().catch(reportFailure),
    };
    return [...groups, [steps]];
  };

  /** A topic's menu, for a right click on its row. */
  const topicMenu = (machineId: string, topicId: string): MenuGroups => {
    const target = topics.find((t) => t.machineId === machineId && t.topic.id === topicId);
    return target ? asMenu(topicActions(target)) : [];
  };

  // A device that drops off while its settings are open stays in the list until they close.
  const settingsNav: SettingsNav = {
    groups: settingsGroups(
      appSections(settings?.appearance.pages ?? []),
      devices.flatMap((device) =>
        device.state && (isOnline(device) || device === settingsDevice)
          ? [{ machineId: device.state.machine.id, name: device.state.machine.name }]
          : [],
      ),
    ),
    onGo: show,
    onClose: closeSettings,
  };
  // Registered on every render, so the handlers always see the current state.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      // A sheet or a dropdown owns the keyboard, and so does a screen's own sheet.
      if (sheet || overlayOpen()) return;

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
        ["global.new_tab", newTab],
        ["global.next_request", nextRequest],
        ["global.interrupt", interrupt],
        ["global.close_tab", () => closeTab()],
        ["global.prev_tab", () => moveTab(-1)],
        ["global.next_tab", () => moveTab(1)],
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
          appMenu={asMenu(appActions())}
          topicMenu={topicMenu}
          onSettings={openSettings}
          onAppSettings={() => openAppSettings()}
          onAddWorkspace={addWorkspaceOn}
          onNewTopic={createTopicIn}
        />
      )}
      {view?.kind === "app_settings" ? (
        <AppSettings
          section={view.section}
          nav={settingsNav}
          devices={devices}
          onAddDevice={() => setSheet({ kind: "add_device" })}
          onPair={localDevice && (() => void pairWith(localDevice).catch(reportFailure))}
          onForget={(device) => void forget(device).catch(reportFailure)}
          onOpenDevice={(device) => device.state && openSettings(device.state.machine.id)}
        />
      ) : settingsDevice && view?.kind === "device_settings" ? (
        <DeviceSettings
          // Each device's sections keep whatever was being typed in them.
          key={view.machineId}
          device={settingsDevice}
          machineId={view.machineId}
          section={view.section}
          nav={settingsNav}
          onPair={() => void pairWith(settingsDevice).catch(reportFailure)}
          onForget={() => void forget(settingsDevice).catch(reportFailure)}
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
                const back = navigation.beforeNewTopic;
                const gone =
                  back?.kind === "workspace"
                    ? !devices.some(
                        (d) =>
                          d.state?.machine.id === back.machineId &&
                          d.state.workspaces.some((w) => w.id === back.workspaceId),
                      )
                    : back?.kind !== "topic" ||
                      !topics.some(
                        (t) => t.machineId === back.machineId && t.topic.id === back.topicId,
                      );
                // With no topic to go back to, the screen stays and keeps its draft.
                if (gone) {
                  focusSidebar();
                  return;
                }
                setNewTopicForm({});
                open(back);
              }}
              onStarted={onTopicStarted}
              onStalled={onTopicStalled}
            />
          ) : view?.kind === "workspace" && pageDevice && pageWorkspace ? (
            <>
              <DeviceNoticeLine device={pageDevice} onReconnect={reconnectNow} />
              {error && <p className="error main-error">{error}</p>}
              <WorkspacePage
                key={`${view.machineId}/${view.workspaceId}`}
                device={pageDevice}
                workspace={pageWorkspace}
                onOpenTopic={(topicId) => open({ kind: "topic", machineId: view.machineId, topicId })}
                onNewTopic={() => createTopicIn(pageDevice, pageWorkspace)}
              />
            </>
          ) : entry ? (
            <>
              <div className="tab-bar">
                <div className="tabs" role="tablist">
                  {tabIds.map((id, index) => {
                    const s = sessions.find((other) => other.id === id);
                    const tab = tabs.find((other) => other.id === id);
                    const label = tab ? tabLabel(tab) : "New tab";
                    const shown = id === shownTabId;
                    return (
                      <div key={id} className={`tab ${shown ? "shown" : ""}`}>
                        <button
                          role="tab"
                          aria-selected={shown}
                          className="tab-open"
                          title={index < 9 ? keysLabel(TAB_ACTIONS[index]) : undefined}
                          onClick={() => selectTab(entry.topic.id, id)}
                        >
                          {s && <StatusIcon state={sessionState(s, isOnline(entry.device))} />}
                          {tab?.kind === "terminal" && <Icon name="terminal" />}
                          {tab?.kind === "diff" && <Icon name="diff" />}
                          <span className="tab-label">{label}</span>
                        </button>
                        {canCloseTab(id) && (
                          <button
                            className="tab-close"
                            tabIndex={-1}
                            // Its key closes the tab that's showing, so only that one names it.
                            title={shown ? titled("Close", "global.close_tab") : "Close"}
                            aria-label={`Close ${label}`}
                            // It goes away with its tab, which would leave focus nowhere.
                            onMouseDown={(event) => event.preventDefault()}
                            onClick={() => closeTab(id)}
                          >
                            <Icon name="remove" />
                          </button>
                        )}
                      </div>
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
                <MenuButton
                  label="Topic actions"
                  className="icon-button"
                  groups={tabBarMenu(entry)}
                  title={
                    isOnline(entry.device)
                      ? `Actions for ${entry.topic.name}`
                      : `${entry.device.state?.machine.name ?? entry.device.url} is offline`
                  }
                >
                  <Icon name="more" />
                </MenuButton>
              </div>
              <DeviceNoticeLine device={entry.device} onReconnect={reconnectNow} />
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
                  draftImages={entry.device.state?.draft_images?.[session.id] ?? NO_IMAGES}
                  commands={entry.device.state?.commands?.[session.id]}
                  worktree={topicWorktree(entry.topic)}
                  onInterrupt={isActive(session) ? interrupt : undefined}
                  onBranched={(id) => selectTab(entry.topic.id, id)}
                />
              ) : shownTerminal ? (
                <Suspense fallback={<div className="pane terminal" />}>
                  <TerminalPane
                    key={shownTerminal.id}
                    machineId={entry.machineId}
                    terminal={shownTerminal}
                    online={isOnline(entry.device)}
                  />
                </Suspense>
              ) : shownDiff ? (
                <DiffPane
                  key={shownDiff.id}
                  machineId={entry.machineId}
                  topic={entry.topic}
                  diff={shownDiff}
                  online={isOnline(entry.device)}
                  onConfirm={setSheet}
                />
              ) : (
                <NewTab
                  key={entry.topic.id}
                  entry={entry}
                  online={isOnline(entry.device)}
                  state={newTabStates[entry.topic.id] ?? NEW_TAB_STATE}
                  onState={(state) => setNewTabState(entry.topic.id, state)}
                  onSettings={openSettings}
                  onTerminal={openTerminal}
                  onDiff={openDiff}
                  onImport={importConversation}
                  onStarted={(sessionId) => onTabStarted(entry.topic.id, sessionId)}
                  onStalled={(sessionId, reason) =>
                    onTabStalled(entry.topic.id, sessionId, reason)
                  }
                />
              )}
            </>
          ) : (
            <Welcome
              devices={devices}
              topics={topics}
              onAddWorkspace={firstOnline && (() => addWorkspaceOn(firstOnline))}
            />
          )}
        </main>
      )}
      {sheet && (
        <Sheets
          sheet={sheet}
          workspaces={
            ("machineId" in sheet
              ? devices.find((d) => d.state?.machine.id === sheet.machineId)?.state?.workspaces
              : undefined) ?? []
          }
          mcpServers={
            ("machineId" in sheet
              ? devices.find((d) => d.state?.machine.id === sheet.machineId)?.state?.mcp_servers
              : undefined) ?? []
          }
          pairingRequests={
            ("machineId" in sheet
              ? devices.find((d) => d.state?.machine.id === sheet.machineId)?.state
                  ?.pairing_requests
              : undefined) ?? []
          }
          onClose={() => setSheet(null)}
          onDone={(output) => {
            if (output.output === "workspace" && sheet.kind === "workspace") {
              open(landingIn(navigation, sheet.machineId, output.workspace));
            }
            // The tab can already be gone by now, so this can't wait for the change.
            if (sheet.kind === "close") focusPane();
            if (sheet.kind === "archive") afterArchiving(sheet);
          }}
        />
      )}
      <Toasts />
    </div>
  );
}

/** Says so while the shown topic's device is away, slow to answer or not answering, so a key
 * that seems to do nothing has its reason on screen. */
function DeviceNoticeLine({ device, onReconnect }: { device: Device; onReconnect: () => void }) {
  const waiting = useWaiting(device.state?.machine.id);
  const notice = deviceNotice(device, waiting);
  if (!notice) return null;
  return (
    <p className="main-notice" role="status" title={notice.detail}>
      <span>{notice.text}</span>
      {notice.reconnect && (
        // Focus stays in the pane, where the keys are.
        <button
          className="button"
          onMouseDown={(event) => event.preventDefault()}
          onClick={onReconnect}
        >
          Reconnect
        </button>
      )}
    </p>
  );
}

function Empty({ children }: { children: React.ReactNode }) {
  return <div className="pane empty">{children}</div>;
}

/** The window with nothing open says what to do next, with the button that does it. */
function Welcome({
  devices,
  topics,
  onAddWorkspace,
}: {
  devices: Device[];
  topics: TopicEntry[];
  onAddWorkspace?: () => void;
}) {
  if (!devices.some(isOnline)) {
    const failure = devices
      .map(connectionProblem)
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
        {onAddWorkspace && (
          <button className="button primary" onClick={onAddWorkspace}>
            Add a workspace
          </button>
        )}
      </Empty>
    );
  }
  return (
    <Empty>
      <p>Choose a topic in the sidebar</p>
    </Empty>
  );
}
