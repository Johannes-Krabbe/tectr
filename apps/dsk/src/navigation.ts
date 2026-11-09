/** A screen of the app. */
export type View =
  | { kind: "topic"; machineId: string; topicId: string }
  | { kind: "new_topic"; machineId: string; workspaceId: string }
  | { kind: "device_settings"; machineId: string; section?: string }
  | { kind: "app_settings"; section: string };

/** What this client is looking at. The daemon owns what exists, and each client chooses what to
 * look at, so moving around on one doesn't move the others. It's the one thing a client
 * remembers: losing it only means opening on the empty state. */
export interface Navigation {
  view: View | null;
  /** The topic cancelling the new topic screen goes back to. Without one, it goes to the sidebar. */
  beforeNewTopic: View | null;
  /** Where closing goes from settings. */
  beforeSettings: View | null;
  /** The section settings opens at. */
  settingsSection: string;
  /** Where moving between workspaces comes back to in each one. Topic ID by workspace ID. */
  lastTopics: Record<string, string>;
  /** Which tab each topic shows. Session ID, or `NEW_TAB`, by topic ID. */
  tabs: Record<string, string>;
  /** The topics with a new tab open. */
  newTabs: string[];
}

/** The ID of a topic's new tab, where you choose what to open. Only this client has it: it
 * becomes a real tab, which every client sees, once something starts from it. */
export const NEW_TAB = "new";

const NOTHING_YET: Navigation = {
  view: null,
  beforeNewTopic: null,
  beforeSettings: null,
  settingsSection: "general",
  lastTopics: {},
  tabs: {},
  newTabs: [],
};

const STORAGE_KEY = "tectr.navigation";

/** What was remembered, or a new client's empty state if that's missing or unreadable. */
export function parseNavigation(stored: string | null): Navigation {
  try {
    const parsed: unknown = JSON.parse(stored ?? "null");
    return parsed && typeof parsed === "object"
      ? { ...NOTHING_YET, ...(parsed as Partial<Navigation>) }
      : NOTHING_YET;
  } catch {
    return NOTHING_YET;
  }
}

export const loadNavigation = () => parseNavigation(localStorage.getItem(STORAGE_KEY));

export function saveNavigation(navigation: Navigation) {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(navigation));
}

const isSettings = (view: View | null) =>
  view?.kind === "device_settings" || view?.kind === "app_settings";

/** Shows a view. `workspaceId` is the workspace it's in, if it's a topic. */
export function goTo(navigation: Navigation, next: View | null, workspaceId?: string): Navigation {
  const from = navigation.view;
  return {
    ...navigation,
    view: next,
    // Moving within these screens doesn't change where leaving them goes back to, and neither
    // does a trip to settings and back.
    beforeNewTopic:
      next?.kind === "new_topic" && from?.kind !== "new_topic" && !isSettings(from)
        ? from?.kind === "topic"
          ? from
          : null
        : navigation.beforeNewTopic,
    beforeSettings: isSettings(next) && !isSettings(from) ? from : navigation.beforeSettings,
    settingsSection: next?.kind === "app_settings" ? next.section : navigation.settingsSection,
    lastTopics:
      next?.kind === "topic" && workspaceId
        ? { ...navigation.lastTopics, [workspaceId]: next.topicId }
        : navigation.lastTopics,
  };
}

/** Where going to a workspace lands: the topic last open in it here, or else its newest. A
 * workspace with no topics has only one thing to do next. */
export function landingIn(
  navigation: Navigation,
  machineId: string,
  workspace: { id: string; topics: { id: string }[] },
): View {
  const { id: workspaceId, topics } = workspace;
  const topic = topics.find((t) => t.id === navigation.lastTopics[workspaceId]) ?? topics.at(-1);
  return topic
    ? { kind: "topic", machineId, topicId: topic.id }
    : { kind: "new_topic", machineId, workspaceId };
}

/** Where archiving the shown topic lands: the topic after it, or else the one before. `workspace`
 * is as it was before archiving. */
export function landingAfterArchiving(
  machineId: string,
  workspace: { id: string; topics: { id: string }[] },
  topicId: string,
): View {
  const { id: workspaceId, topics } = workspace;
  const index = topics.findIndex((t) => t.id === topicId);
  const topic = topics[index + 1] ?? topics[index - 1];
  return topic
    ? { kind: "topic", machineId, topicId: topic.id }
    : { kind: "new_topic", machineId, workspaceId };
}

export function chooseTab(navigation: Navigation, topicId: string, tabId: string): Navigation {
  return { ...navigation, tabs: { ...navigation.tabs, [topicId]: tabId } };
}

/** A topic's tabs: its sessions, then the new tab while it's open. A topic with no sessions
 * always has one, because choosing what to open is all there is to do there. */
export function topicTabs(navigation: Navigation, topicId: string, sessionIds: string[]): string[] {
  const open = sessionIds.length === 0 || navigation.newTabs.includes(topicId);
  return open ? [...sessionIds, NEW_TAB] : sessionIds;
}

/** Opens the topic's new tab, or goes to it when it's already open. */
export function openNewTab(navigation: Navigation, topicId: string): Navigation {
  const newTabs = navigation.newTabs.includes(topicId)
    ? navigation.newTabs
    : [...navigation.newTabs, topicId];
  return chooseTab({ ...navigation, newTabs }, topicId, NEW_TAB);
}

/** Closes the topic's new tab. Which tab shows next is up to `shownTab`. */
export function closeNewTab(navigation: Navigation, topicId: string): Navigation {
  return { ...navigation, newTabs: navigation.newTabs.filter((id) => id !== topicId) };
}

/** The tab a topic shows: the chosen one while it exists. A closed tab, which any client can
 * do, gives way to the one after it in `before`, the tabs as they last were, or else the one
 * before it. With nothing to go on, it's the newest. */
export function shownTab(
  tabs: string[],
  chosen: string | undefined,
  before: string[],
): string | undefined {
  if (chosen !== undefined && tabs.includes(chosen)) return chosen;
  const index = chosen === undefined ? -1 : before.indexOf(chosen);
  const remains = (id: string) => tabs.includes(id);
  const neighbor =
    index === -1
      ? undefined
      : (before.slice(index + 1).find(remains) ?? before.slice(0, index).reverse().find(remains));
  return neighbor ?? tabs[tabs.length - 1];
}
