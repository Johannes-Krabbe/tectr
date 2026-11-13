import { useEffect, useState } from "react";

/** The time, kept current every `everyMs` while `ticking`, for text that counts down. It
 * doesn't tick otherwise, so a screen with nothing counting doesn't redraw. */
export function useNow(ticking: boolean, everyMs = 30_000): number {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!ticking) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), everyMs);
    return () => clearInterval(timer);
  }, [ticking, everyMs]);
  return now;
}
