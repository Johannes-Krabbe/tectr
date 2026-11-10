// Commands on their way to a device. A command sent again before its answer is back is the
// same press of a key or a button, so it goes out once and every sender gets that one answer.
// Otherwise a device that's slow to answer gets ten terminals for ten impatient presses.

/** Asked for in passing while something else is shown, so nobody is waiting on them. */
const IN_PASSING = new Set(["read_image", "list_folders"]);

interface Flying<O> {
  machineId: string;
  /** Left out for what's asked in passing. */
  since: number | null;
  answer: Promise<O>;
}

export interface Requests<C, O> {
  request(machineId: string, command: C): Promise<O>;
  /** When the oldest command a device still hasn't answered went out, or null with none. */
  waitingSince(machineId: string): number | null;
  /** Calls back whenever a command goes out or is answered. Returns what stops it. */
  subscribe(notify: () => void): () => void;
}

export function trackRequests<C extends { command: string }, O>(
  send: (machineId: string, command: C) => Promise<O>,
  now: () => number = Date.now,
): Requests<C, O> {
  const flying = new Map<string, Flying<O>>();
  const subscribers = new Set<() => void>();
  const changed = () => {
    for (const notify of subscribers) notify();
  };

  return {
    request(machineId, command) {
      const key = `${machineId}\n${JSON.stringify(command)}`;
      const same = flying.get(key);
      if (same) return same.answer;
      const answer = send(machineId, command).finally(() => {
        flying.delete(key);
        changed();
      });
      const since = IN_PASSING.has(command.command) ? null : now();
      flying.set(key, { machineId, since, answer });
      changed();
      return answer;
    },
    waitingSince(machineId) {
      let oldest: number | null = null;
      for (const one of flying.values()) {
        if (one.machineId !== machineId || one.since === null) continue;
        if (oldest === null || one.since < oldest) oldest = one.since;
      }
      return oldest;
    },
    subscribe(notify) {
      subscribers.add(notify);
      return () => subscribers.delete(notify);
    },
  };
}
