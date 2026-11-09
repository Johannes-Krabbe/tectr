import {
  Bot,
  GitCompare,
  type LucideIcon,
  Minus,
  Plus,
  Settings,
  Square,
  SquareTerminal,
} from "lucide-react";

// The only file that imports lucide-react, which a test checks. Swapping one icon, or drawing
// one by hand, is a change here and nowhere else. Status icons are separate: see StatusIcon.
const ICONS = {
  add: { glyph: Plus },
  agent: { glyph: Bot },
  diff: { glyph: GitCompare },
  lower: { glyph: Minus },
  raise: { glyph: Plus },
  settings: { glyph: Settings },
  stop: { glyph: Square, filled: true },
  terminal: { glyph: SquareTerminal },
} satisfies Record<string, { glyph: LucideIcon; filled?: boolean }>;

export type IconName = keyof typeof ICONS;

/** Sized by `size.icon` in CSS. The stroke stays 1.5px at any size, so icons match. */
export function Icon({ name }: { name: IconName }) {
  const icon: { glyph: LucideIcon; filled?: boolean } = ICONS[name];
  const Glyph = icon.glyph;
  return (
    <Glyph
      className="icon"
      aria-hidden="true"
      strokeWidth={1.5}
      nonScalingStroke
      fill={icon.filled ? "currentColor" : "none"}
    />
  );
}
