// Results that need nothing from you: a line that's up for a few seconds, then gone. What
// failed doesn't go here, because it has to stay until someone has dealt with it.

import { useSyncExternalStore } from "react";

export interface Toast {
  id: number;
  text: string;
}

/** More than this at once and the oldest goes early, so they never pile up the window. */
const MOST = 3;
/** How long one is up when the settings haven't said. */
export const TOAST_SECONDS = 3;

let toasts: Toast[] = [];
let nextId = 1;
const timers = new Map<number, ReturnType<typeof setTimeout>>();
const subscribers = new Set<() => void>();

function publish(next: Toast[]) {
  for (const gone of toasts.filter((toast) => !next.includes(toast))) {
    clearTimeout(timers.get(gone.id));
    timers.delete(gone.id);
  }
  toasts = next;
  for (const notify of subscribers) notify();
}

/** Shows a line for a few seconds. Saying what's already up again keeps it as one line and
 * starts its time over. `notify` in keybinds.ts is this with the time the settings give. */
export function showToast(text: string, seconds = TOAST_SECONDS) {
  const toast = { id: nextId++, text };
  publish([...toasts.filter((other) => other.text !== text), toast].slice(-MOST));
  timers.set(
    toast.id,
    setTimeout(() => publish(toasts.filter((other) => other !== toast)), seconds * 1000),
  );
}

export const shownToasts = () => toasts;

export function useToasts(): Toast[] {
  return useSyncExternalStore((notify) => {
    subscribers.add(notify);
    return () => subscribers.delete(notify);
  }, shownToasts);
}
