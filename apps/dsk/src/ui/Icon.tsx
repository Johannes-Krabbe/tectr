import {
  Bot,
  Check,
  ChevronDown,
  ChevronRight,
  Clock,
  Copy,
  Ellipsis,
  GitFork,
  GitCompare,
  History,
  type LucideIcon,
  Minus,
  Plus,
  Search,
  Settings,
  Square,
  SquareTerminal,
  X,
} from "lucide-react";

// The only file that imports lucide-react, which a test checks. Swapping one icon, or drawing
// one by hand, is a change here and nowhere else. Status icons are separate: see StatusIcon.
const ICONS = {
  add: { glyph: Plus },
  agent: { glyph: Bot },
  branch: { glyph: GitFork },
  copy: { glyph: Copy },
  diff: { glyph: GitCompare },
  expires: { glyph: Clock },
  fold: { glyph: ChevronDown },
  import: { glyph: History },
  lower: { glyph: Minus },
  more: { glyph: Ellipsis },
  raise: { glyph: Plus },
  remove: { glyph: X },
  search: { glyph: Search },
  settings: { glyph: Settings },
  stop: { glyph: Square, filled: true },
  terminal: { glyph: SquareTerminal },
  unfold: { glyph: ChevronRight },
  viewed: { glyph: Check },
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
