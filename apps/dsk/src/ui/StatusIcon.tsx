import { useLayoutEffect, useRef } from "react";
import done from "../../../../design/icons/done.svg?raw";
import failed from "../../../../design/icons/failed.svg?raw";
import idle from "../../../../design/icons/idle.svg?raw";
import read from "../../../../design/icons/read.svg?raw";
import stale from "../../../../design/icons/stale.svg?raw";
import waiting from "../../../../design/icons/waiting.svg?raw";
import working from "../../../../design/icons/working.svg?raw";
import { type ActionId, keysLabel } from "../keybinds";
import { STATE_LABEL, type StatusState } from "../model";

const ICONS: Record<StatusState, string> = { done, failed, idle, read, stale, waiting, working };

// Every state is a layer of one SVG, and CSS shows the current one. The element stays the same
// when the state changes, so it can move from one to the next: see `.status-icon` in app.css.
const inside = (svg: string) => svg.replace(/^<svg[^>]*>/, "").replace(/<\/svg>\s*$/, "");
const MARKUP = `<svg viewBox="0 0 14 14" aria-hidden="true">${Object.entries(ICONS)
  .map(([state, svg]) => `<g class="layer layer-${state}">${inside(svg)}</g>`)
  .join("")}</svg>`;

/** Starts a turning icon in step with every other one, however late it appeared, so a column of
 * them reads as one signal instead of jitter. */
function inStep(element: HTMLElement) {
  for (const animation of element.getAnimations({ subtree: true })) {
    if (animation instanceof CSSAnimation) animation.startTime = 0;
  }
}

export function StatusIcon({ state }: { state: StatusState }) {
  const ref = useRef<HTMLSpanElement>(null);
  useLayoutEffect(() => {
    if (state === "working" && ref.current) inStep(ref.current);
  }, [state]);
  return (
    <span
      ref={ref}
      className="status-icon"
      data-state={state}
      role="img"
      aria-label={STATE_LABEL[state]}
      // Static files from design/icons, never anything from an agent.
      dangerouslySetInnerHTML={{ __html: MARKUP }}
    />
  );
}

export function Kbd({ children }: { children: string }) {
  return <kbd className="kbd">{children}</kbd>;
}

/** An action's key as it's currently set, and nothing when it has none. */
export function Keys({ action }: { action: ActionId }) {
  const label = keysLabel(action);
  return label ? <Kbd>{label}</Kbd> : null;
}
