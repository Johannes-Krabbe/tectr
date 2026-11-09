// How typing a path on another machine behaves: which folder to ask that machine about, what
// to suggest from its answer, and what the keys do. The webview can't see that machine's disk.

import type { Folder } from "./engine";

/** What the input opens with, so the home folder's contents are there to choose from. */
export const STARTING_PATH = "~/";

export interface Suggestion {
  /** As it would be typed, which also keeps a typed `~` as it was. */
  typed: string;
  /** Absolute, as the daemon reported it. */
  path: string;
  tag: "added" | "git" | null;
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
      tag: workspacePaths.includes(path) ? "added" : is_repo ? "git" : null,
    }));
}

/** Typing part of a name offers the first match. A finished folder offers nothing, which is
 * what lets the next Enter add it. */
export const highlightWhileTyping = (partial: string | undefined): number | null =>
  partial ? 0 : null;

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
