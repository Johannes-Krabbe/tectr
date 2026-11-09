import { useEffect, useRef, useState } from "react";

/** How long typing waits before telling the daemon, so it isn't told every keystroke. */
const PUSH_AFTER_MS = 150;
/** How long after typing an update from the daemon is ignored. It's only the echo of what
 * was typed a moment ago, and applying it would take back the newest letters. */
const ECHO_MS = 500;

/** Text that belongs to the daemon, while it's being typed here. What's on screen can only
 * be ahead of the daemon by the last moment of typing. */
export function useSyncedText(remote: string, push: (text: string) => void) {
  const [text, setText] = useState(remote);
  const latest = useRef({ text: remote, push, editedAt: 0 });
  latest.current.push = push;
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  const flush = () => {
    if (timer.current === undefined) return;
    clearTimeout(timer.current);
    timer.current = undefined;
    latest.current.push(latest.current.text);
  };

  const show = (next: string) => {
    latest.current.text = next;
    setText(next);
  };

  useEffect(() => {
    if (Date.now() - latest.current.editedAt >= ECHO_MS) show(remote);
  }, [remote]);

  // Leaving the screen shouldn't lose the last moment of typing.
  useEffect(() => flush, []);

  return {
    text,
    change(next: string) {
      show(next);
      latest.current.editedAt = Date.now();
      clearTimeout(timer.current);
      timer.current = setTimeout(flush, PUSH_AFTER_MS);
    },
    /** Stops a push that hasn't happened, because what it would say is about to be sent. */
    cancel() {
      clearTimeout(timer.current);
      timer.current = undefined;
    },
    /** It went where it was going, so there's nothing left here. */
    clear() {
      this.cancel();
      latest.current.editedAt = Date.now();
      show("");
    },
  };
}
