// How the transcript moves: messages typing out, and scrolling that eases into place.

/** The slowest a message types, which on a 60 Hz screen is one letter per frame. */
const MIN_LETTERS_PER_S = 60;
/** How far behind the stream typing settles. The rate follows what's left, so a fast agent
 * or a big chunk can't leave the text further and further behind. */
const CATCH_UP_S = 0.5;
/** A line's worth of scrolling settles in about `motion.normal`. */
const SCROLL_EASE_MS = 30;
/** How close to the bottom still counts as following along, in pixels. */
export const FOLLOW_WITHIN = 40;

/** How much of a message shows after this much time. It's fractional, so that letters still
 * come at the right rate when a frame is too short for a whole one. */
export function nextTyped(shown: number, length: number, elapsedMs: number): number {
  const behind = length - shown;
  if (behind <= 0) return length;
  const rate = Math.max(MIN_LETTERS_PER_S, behind / CATCH_UP_S);
  return Math.min(length, shown + (rate * Math.max(0, elapsedMs)) / 1000);
}

/** The first `shown` letters, never stopping halfway through an emoji's surrogate pair. */
export function typedText(text: string, shown: number): string {
  const code = text.charCodeAt(shown - 1);
  return text.slice(0, code >= 0xd800 && code <= 0xdbff ? shown + 1 : shown);
}

export function easedScroll(at: number, target: number, elapsedMs: number): number {
  const next = at + (target - at) * (1 - Math.exp(-Math.max(0, elapsedMs) / SCROLL_EASE_MS));
  return Math.abs(target - next) < 0.5 ? target : next;
}

/** Whether to keep following the end of the text after a scroll. Being far from the bottom
 * isn't enough to stop: a table or code block can make the text jump ahead of the scroll
 * position between two frames. Only moving up is leaving. */
export function keepFollowing(following: boolean, distance: number, movedUp: boolean): boolean {
  if (distance < FOLLOW_WITHIN) return true;
  return movedUp ? false : following;
}

export interface Scroller {
  /** Eases to a position, or carries on to it from wherever it's already moving. */
  to(top: number): void;
  jump(top: number): void;
  stop(): void;
  moving(): boolean;
  /** Where it's going, so that repeated presses add up instead of restarting from here. */
  headedTo(): number;
}

export function newScroller(element: () => HTMLElement | null, animated: () => boolean): Scroller {
  let frame = 0;
  let at = 0;
  let target = 0;
  let last = 0;

  const stop = () => {
    cancelAnimationFrame(frame);
    frame = 0;
  };

  const tick = (now: number) => {
    const el = element();
    if (!el) {
      frame = 0;
      return;
    }
    at = easedScroll(at, target, now - last);
    last = now;
    el.scrollTop = at;
    frame = at === target ? 0 : requestAnimationFrame(tick);
  };

  const jump = (top: number) => {
    stop();
    const el = element();
    if (el) el.scrollTop = top;
  };

  return {
    jump,
    stop,
    moving: () => frame !== 0,
    headedTo: () => (frame !== 0 ? target : (element()?.scrollTop ?? 0)),
    to(top) {
      const el = element();
      if (!el) return;
      target = Math.min(Math.max(top, 0), el.scrollHeight - el.clientHeight);
      if (!animated()) jump(target);
      else if (frame === 0 && el.scrollTop !== target) {
        at = el.scrollTop;
        last = performance.now();
        frame = requestAnimationFrame(tick);
      }
    },
  };
}
