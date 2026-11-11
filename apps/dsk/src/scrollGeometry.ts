// Where the app's scroll bar goes. Pure, so it's tested.

/** The gap between a bar and its area's edge, and the bar's thickness. */
const INSET = 2;
const THICKNESS = 6;
/** A long page still gets a thumb big enough to grab. */
const MIN_LENGTH = 24;

export interface Box {
  left: number;
  top: number;
  width: number;
  height: number;
}

export interface Thumb extends Box {
  /** How far the content moves for each pixel the thumb is dragged. */
  scrollPerPixel: number;
}

/** A thumb along the right edge for `y`, or the bottom edge for `x`, sized by how much of the
 * content shows and placed by how far it's scrolled. */
export function scrollThumb(
  axis: "y" | "x",
  area: Box,
  scroll: { client: number; size: number; offset: number },
): Thumb {
  const track = (axis === "y" ? area.height : area.width) - 2 * INSET;
  const length = Math.min(track, Math.max(MIN_LENGTH, (track * scroll.client) / scroll.size));
  const range = Math.max(1, scroll.size - scroll.client);
  const travel = track - length;
  const along = INSET + (travel * Math.min(Math.max(scroll.offset, 0), range)) / range;
  const scrollPerPixel = travel > 0 ? range / travel : 0;
  return axis === "y"
    ? {
        left: area.left + area.width - INSET - THICKNESS,
        top: area.top + along,
        width: THICKNESS,
        height: length,
        scrollPerPixel,
      }
    : {
        left: area.left + along,
        top: area.top + area.height - INSET - THICKNESS,
        width: length,
        height: THICKNESS,
        scrollPerPixel,
      };
}
