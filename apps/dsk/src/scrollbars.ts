// The app's own scroll bar, for appearance.layout.scrollbars. The web view doesn't redraw a
// styled native scroll bar when only its class changes, so native ones are hidden in CSS and
// one floating bar is drawn over whatever was last scrolled or pointed at. It shows while
// that scrolls, while the pointer is near its edge, and while it's dragged.

import { scrollThumb } from "./scrollGeometry";

/** How long a bar stays after the last movement, about what macOS does. */
const SHOWN_AFTER_MS = 1000;
/** How close to a scrolling area's edge the pointer has to be to bring its bar up. */
const NEAR_EDGE_PX = 14;

type Axis = "y" | "x";
const AXES: Axis[] = ["y", "x"];

const on = () => document.documentElement.dataset.scrollbars === "auto";

function scrolls(el: Element, axis: Axis): boolean {
  const overflowing =
    axis === "y" ? el.scrollHeight > el.clientHeight + 1 : el.scrollWidth > el.clientWidth + 1;
  if (!overflowing) return false;
  const style = getComputedStyle(el);
  const overflow = axis === "y" ? style.overflowY : style.overflowX;
  return overflow === "auto" || overflow === "scroll";
}

/** The nearest area under the pointer that scrolls, with the pointer close to its edge. */
function nearEdge(target: EventTarget | null, x: number, y: number): [Element, Axis] | null {
  for (let el = target instanceof Element ? target : null; el; el = el.parentElement) {
    const rect = el.getBoundingClientRect();
    if (rect.right - x <= NEAR_EDGE_PX && scrolls(el, "y")) return [el, "y"];
    if (rect.bottom - y <= NEAR_EDGE_PX && scrolls(el, "x")) return [el, "x"];
  }
  return null;
}

export function followScrolling() {
  const bars: Record<Axis, HTMLDivElement> = { y: thumb("y"), x: thumb("x") };
  let target: Element | null = null;
  let hideTimer: ReturnType<typeof setTimeout> | undefined;
  let held = false;

  const place = () => {
    for (const axis of AXES) {
      const bar = bars[axis];
      const geometry = target && scrolls(target, axis) ? measure(target, axis) : null;
      bar.classList.toggle("shown", geometry !== null);
      if (!geometry) continue;
      bar.style.left = `${geometry.left}px`;
      bar.style.top = `${geometry.top}px`;
      bar.style.width = `${geometry.width}px`;
      bar.style.height = `${geometry.height}px`;
    }
  };

  const hide = () => {
    if (held) return;
    bars.y.classList.remove("shown");
    bars.x.classList.remove("shown");
  };

  const show = (el: Element) => {
    target = el;
    place();
    clearTimeout(hideTimer);
    hideTimer = setTimeout(hide, SHOWN_AFTER_MS);
  };

  document.addEventListener(
    "scroll",
    (event) => {
      if (!on() || !(event.target instanceof Element)) return;
      show(event.target);
    },
    { capture: true, passive: true },
  );

  let pending: PointerEvent | null = null;
  document.addEventListener(
    "pointermove",
    (event) => {
      if (!on() || held) return;
      // Looking at styles on every move adds up, so once a frame is enough.
      if (pending === null) {
        requestAnimationFrame(() => {
          const last = pending;
          pending = null;
          if (!last || Object.values(bars).some((bar) => bar === last.target)) return;
          const found = nearEdge(last.target, last.clientX, last.clientY);
          if (found) show(found[0]);
        });
      }
      pending = event;
    },
    { passive: true },
  );

  for (const axis of AXES) {
    const bar = bars[axis];
    // The bar sits over the content, so wheeling on it still scrolls what's under it.
    bar.addEventListener(
      "wheel",
      (event) => {
        if (!target) return;
        event.preventDefault();
        target.scrollBy(event.deltaX, event.deltaY);
      },
      { passive: false },
    );
    bar.addEventListener("pointerdown", (event) => {
      const el = target;
      if (!el) return;
      event.preventDefault();
      bar.setPointerCapture(event.pointerId);
      held = true;
      bar.classList.add("held");
      const start = axis === "y" ? event.clientY : event.clientX;
      const startScroll = axis === "y" ? el.scrollTop : el.scrollLeft;
      const geometry = measure(el, axis);
      const move = (moved: PointerEvent) => {
        const delta = (axis === "y" ? moved.clientY : moved.clientX) - start;
        const next = startScroll + delta * geometry.scrollPerPixel;
        if (axis === "y") el.scrollTop = next;
        else el.scrollLeft = next;
      };
      const release = () => {
        held = false;
        bar.classList.remove("held");
        bar.removeEventListener("pointermove", move);
        bar.removeEventListener("pointerup", release);
        bar.removeEventListener("pointercancel", release);
        show(el);
      };
      bar.addEventListener("pointermove", move);
      bar.addEventListener("pointerup", release);
      bar.addEventListener("pointercancel", release);
    });
  }
}

function thumb(axis: Axis): HTMLDivElement {
  const bar = document.createElement("div");
  bar.className = `scroll-thumb ${axis}`;
  bar.setAttribute("aria-hidden", "true");
  document.body.append(bar);
  return bar;
}

function measure(el: Element, axis: Axis) {
  const rect = el.getBoundingClientRect();
  return scrollThumb(
    axis,
    { left: rect.left, top: rect.top, width: rect.width, height: rect.height },
    axis === "y"
      ? { client: el.clientHeight, size: el.scrollHeight, offset: el.scrollTop }
      : { client: el.clientWidth, size: el.scrollWidth, offset: el.scrollLeft },
  );
}
