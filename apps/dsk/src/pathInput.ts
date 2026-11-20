// How typing a path on another machine behaves: which folder to ask that machine about, what
// to suggest from its answer, and what the keys do. The webview can't see that machine's disk.

import type { Folder, Repo } from "./engine";

/** What the input opens with, so the home folder's contents are there to choose from. */
export const STARTING_PATH = "~/";

export interface Suggestion {
  /** As it would be typed, which also keeps a typed `~` as it was. */
  typed: string;
  /** Absolute, as the daemon reported it. */
  path: string;
  /** Already a workspace, a closed workspace that adding opens again, just a repo, a GitHub
   * repository to clone there, or a new folder to make with `git init`. */
  tag: "added" | "closed" | "git" | "clone" | "new" | null;
  /** What a clone row clones. */
  repo?: Repo;
}

const canList = (path: string) => path.startsWith("/") || path.startsWith("~/");

/** Splits what's typed into the folder to ask about and the start of a name in it. Null when
 * it isn't a path that can be listed yet. */
export function splitPath(typed: string): { parent: string; partial: string } | null {
  const path = typed.trim();
  if (!canList(path)) return null;
  const cut = path.lastIndexOf("/") + 1;
  return { parent: path.slice(0, cut), partial: path.slice(cut) };
}

/** What to show from a folder's contents. Folders starting with a dot stay out of the way
 * until a dot is typed, as in a shell. */
export function suggest(
  parent: string,
  partial: string,
  folders: Folder[],
  workspacePaths: string[],
  closedPaths: string[] = [],
): Suggestion[] {
  const start = partial.toLowerCase();
  return folders
    .filter(
      ({ name }) =>
        name.toLowerCase().startsWith(start) && (start.startsWith(".") || !name.startsWith(".")),
    )
    .map(({ name, path, is_repo }) => ({
      typed: `${parent}${name}/`,
      path,
      tag: workspacePaths.includes(path)
        ? "added"
        : closedPaths.includes(path)
          ? "closed"
          : is_repo
            ? "git"
            : null,
    }));
}

/** How many repositories to offer at once, so they never bury the folders. Typing more narrows
 * them down. */
export const MOST_CLONES = 5;

/** The device's GitHub repositories whose name starts with what's typed, offered for cloning
 * into the folder being typed in, under its folders. A repository that already has a folder of
 * its name there is left out, since that folder is its row. Nothing typed offers none, or every
 * folder would list the same repositories. */
export function suggestClones(
  parent: string,
  partial: string,
  repos: Repo[],
  folders: Folder[],
  most = MOST_CLONES,
): Suggestion[] {
  if (!partial) return [];
  const start = partial.toLowerCase();
  // Compared ignoring case, because the Mac's disk does.
  const taken = new Set(folders.map((folder) => folder.name.toLowerCase()));
  return repos
    .filter((repo) => {
      const name = repo.name.toLowerCase();
      return name.startsWith(start) && !taken.has(name);
    })
    .slice(0, most)
    .map((repo) => ({ typed: `${parent}${repo.name}`, path: `${parent}${repo.name}`, tag: "clone", repo }));
}

/** Offers to make the typed name a new folder with a fresh repository in it, under the folders
 * and repositories, unless a folder of that name is there already. */
export function suggestNew(parent: string, partial: string, folders: Folder[]): Suggestion[] {
  const name = partial.trim();
  if (!name || name === "." || name === "..") return [];
  const taken = folders.some((folder) => folder.name.toLowerCase() === name.toLowerCase());
  return taken ? [] : [{ typed: `${parent}${name}`, path: `${parent}${name}`, tag: "new" }];
}

/** Typing part of a name offers the first row: a folder starting with it, else a repository to
 * clone, else a new folder. A finished folder offers nothing, which is what lets the next
 * Enter add it. */
export const highlightWhileTyping = (partial: string | undefined): number | null =>
  partial ? 0 : null;

/** Whether Enter goes into the highlighted folder rather than submitting. A folder that's
 * already a workspace, open or closed, has nothing further in to open, so Enter opens it, and
 * a clone's or a new folder's row isn't there yet, so Enter makes it. */
export const completesOnEnter = (chosen: Suggestion | undefined): boolean =>
  chosen !== undefined && (chosen.tag === null || chosen.tag === "git");

/** Moves through the rows, with "nothing highlighted" as one more stop between the last and
 * the first, so what's typed can always be submitted as it is. */
export function moveHighlight(current: number | null, step: 1 | -1, count: number): number | null {
  if (count === 0) return null;
  const stops = count + 1;
  const next = ((current ?? count) + step + stops) % stops;
  return next === count ? null : next;
}

/** A pasted path replaces the starting path. Typing never does, so `~` can't vanish on you. */
export function afterPaste(current: string, pasted: string): string | null {
  const path = pasted.trim();
  return current === STARTING_PATH && (path === "~" || canList(path)) ? path : null;
}

/** Whether a device is this machine, where asking it is as good as instant. */
export function isLoopback(url: string): boolean {
  try {
    const { hostname } = new URL(url);
    return hostname === "localhost" || hostname === "[::1]" || /^127(\.\d+){3}$/.test(hostname);
  } catch {
    return false;
  }
}
