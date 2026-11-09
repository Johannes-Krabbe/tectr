// Turns the appearance settings into what's on screen. The values, themes and what they can
// be come from crates/settings. They end up as CSS variables on <html>, and everything else
// follows from those through design/tokens and the stylesheets.

export type AppearanceValue = boolean | number | string;
export type Scheme = "dark" | "light";

export type FieldKind =
  | { type: "choice"; options: { value: string; label: string }[] }
  | { type: "theme"; scheme: Scheme }
  | { type: "font"; bundled: string; system: string; monospace: boolean }
  | { type: "number"; min: number; max: number; step: number; unit: string }
  | { type: "toggle" };

export interface AppearanceField {
  id: string;
  group: string;
  label: string;
  about: string;
  kind: FieldKind;
  value: AppearanceValue;
  customized: boolean;
}

export interface Theme {
  id: string;
  name: string;
  scheme: Scheme;
  custom: boolean;
  colors: Record<string, string>;
}

export interface Appearance {
  groups: { id: string; title: string }[];
  fields: AppearanceField[];
  themes: Theme[];
}

export interface InstalledFont {
  name: string;
  monospace: boolean;
}

/** A value being tried out in a list, which nothing has saved yet. */
export interface AppearancePreview {
  id: string;
  value: AppearanceValue;
}

export interface Option {
  value: string;
  label: string;
  meta?: string;
  theme?: Theme;
}

function reader(appearance: Appearance, preview: AppearancePreview | null) {
  return (id: string): AppearanceValue | undefined =>
    id === preview?.id ? preview.value : appearance.fields.find((field) => field.id === id)?.value;
}

/** Which theme is showing. Trying out a theme shows it, whichever mode is on. */
export function schemeOf(
  appearance: Appearance,
  systemIsDark: boolean,
  preview: AppearancePreview | null = null,
): Scheme {
  if (preview?.id === "dark_theme") return "dark";
  if (preview?.id === "light_theme") return "light";
  const mode = reader(appearance, preview)("mode");
  if (mode === "dark" || mode === "light") return mode;
  return systemIsDark ? "dark" : "light";
}

/** The theme a preview shows, for what the webview can't draw. Null if it doesn't change it. */
export function previewedTheme(
  appearance: Appearance,
  systemIsDark: boolean,
  preview: AppearancePreview | null,
): string | null {
  if (!preview || !["mode", "dark_theme", "light_theme"].includes(preview.id)) return null;
  const scheme = schemeOf(appearance, systemIsDark, preview);
  return String(reader(appearance, preview)(scheme === "dark" ? "dark_theme" : "light_theme"));
}

// CSS's own names for fonts, which stop meaning that in quotes.
const GENERIC_FAMILIES = new Set([
  "system-ui",
  "ui-monospace",
  "ui-sans-serif",
  "ui-serif",
  "ui-rounded",
  "sans-serif",
  "serif",
  "monospace",
  "cursive",
  "fantasy",
]);

/** The chosen font in front of the bundled ones, for what it doesn't have or if it's gone. */
export function fontStack(family: string, stack: string): string {
  if (!family) return `var(${stack})`;
  const name = GENERIC_FAMILIES.has(family) ? family : `"${family}"`;
  return `${name}, var(${stack})`;
}

/** Every CSS variable the settings control. Nothing here is a value of its own. */
export function cssVariables(
  appearance: Appearance,
  scheme: Scheme,
  preview: AppearancePreview | null = null,
): Record<string, string> {
  const read = reader(appearance, preview);
  const themeId = read(scheme === "dark" ? "dark_theme" : "light_theme");
  const theme = appearance.themes.find((candidate) => candidate.id === themeId);
  const variables: Record<string, string> = {
    "--font-ui": fontStack(String(read("text.family") ?? ""), "--font-ui-stack"),
    "--font-code": fontStack(String(read("code.family") ?? ""), "--font-code-stack"),
    "--font-ui-size": `${read("text.size")}px`,
    "--font-code-size": `${read("code.size")}px`,
    "--font-reading-line-height": String(read("text.line_height")),
    "--font-code-line-height": String(read("code.line_height")),
    "--font-code-ligatures": read("code.ligatures") ? "normal" : "none",
    "--font-smoothing": String(read("text.smoothing")),
    "--density": String(Number(read("layout.density")) / 100),
    "--corners": String(Number(read("layout.corners")) / 100),
    "--layout-sidebar-width": `${read("layout.sidebar_width")}px`,
    "--layout-content-width": `${read("layout.content_width")}px`,
  };
  for (const [token, color] of Object.entries(theme?.colors ?? {})) {
    variables[`--color-${token.replaceAll(".", "-")}`] = color;
  }
  return variables;
}

/** What a setting's list offers, with what it's set to even if that isn't installed. */
export function optionsOf(
  field: AppearanceField,
  themes: Theme[],
  fonts: InstalledFont[] | null,
): Option[] {
  const { kind } = field;
  if (kind.type === "choice") return kind.options;
  if (kind.type === "theme") {
    return themes
      .filter((theme) => theme.scheme === kind.scheme)
      .map((theme) => ({
        value: theme.id,
        label: theme.name,
        meta: theme.custom ? "Yours" : undefined,
        theme,
      }));
  }
  if (kind.type === "font") {
    const installed = (fonts ?? [])
      .slice()
      // Code wants a monospaced font, so those come first there, in the order they came in.
      .sort((a, b) => (kind.monospace ? Number(b.monospace) - Number(a.monospace) : 0))
      .map((font) => ({
        value: font.name,
        label: font.name,
        meta: font.monospace ? "Monospaced" : undefined,
      }));
    const options: Option[] = [
      { value: "", label: kind.bundled, meta: "Comes with tectr" },
      { value: kind.system, label: "System font" },
      ...installed,
    ];
    const current = String(field.value);
    if (fonts && !options.some((option) => option.value === current)) {
      options.splice(2, 0, { value: current, label: current, meta: "Not installed" });
    }
    return options;
  }
  return [];
}

/** One step in a direction, or the same value at the end. Numbers land on whole steps, so
 * a 13.25 from the file goes to 13.5 and then on from there. */
export function stepped(
  field: AppearanceField,
  direction: -1 | 1,
  options: Option[],
): AppearanceValue {
  const { kind, value } = field;
  if (kind.type === "toggle") return !value;
  if (kind.type === "number") {
    const position = Number(value) / kind.step;
    // A little room for 1.55 / 0.05 not coming out whole.
    const whole = direction > 0 ? Math.floor(position + 1e-6) + 1 : Math.ceil(position - 1e-6) - 1;
    const next = Math.round(whole * kind.step * 1000) / 1000;
    return Math.min(Math.max(next, kind.min), kind.max);
  }
  const index = options.findIndex((option) => option.value === value);
  return options[index + direction]?.value ?? value;
}

export function formatValue(field: AppearanceField): string {
  const { kind, value } = field;
  if (kind.type === "toggle") return value ? "On" : "Off";
  if (kind.type === "number") return `${value}${kind.unit}`;
  return String(value);
}

const STORAGE_KEY = "tectr.appearance";

let shown: Appearance | null = null;
let preview: AppearancePreview | null = null;

export const systemIsDark = () => window.matchMedia("(prefers-color-scheme: dark)").matches;

function paint() {
  if (!shown) return;
  const scheme = schemeOf(shown, systemIsDark(), preview);
  const root = document.documentElement;
  root.dataset.theme = scheme;
  // Reduce Motion is handled in CSS, so it applies even before the settings arrive.
  root.dataset.statusMotion =
    reader(shown, preview)("layout.animate_status") === false ? "off" : "on";
  for (const [name, value] of Object.entries(cssVariables(shown, scheme, preview))) {
    root.style.setProperty(name, value);
  }
}

/** Puts the settings on screen, and remembers them for the next launch. */
export function showAppearance(appearance: Appearance) {
  shown = appearance;
  paint();
  localStorage.setItem(STORAGE_KEY, JSON.stringify(appearance));
}

/** A setting as it stands, which until the real ones arrive is what the last launch had. */
export const shownValue = (id: string): AppearanceValue | undefined =>
  shown?.fields.find((field) => field.id === id)?.value;

/** Whether things should move: the setting, and the system's Reduce Motion. */
export const motionAllowed = () =>
  shownValue("transcript.animate") !== false &&
  !window.matchMedia("(prefers-reduced-motion: reduce)").matches;

/** Shows a value in the webview without saving it. Null goes back to the settings. */
export function previewAppearance(next: AppearancePreview | null) {
  preview = next;
  paint();
}

/** Paints what the last launch had while the real settings are on their way, and follows the
 * system from then on. Anything wrong with what was remembered just means the defaults. */
export function restoreAppearance() {
  window
    .matchMedia("(prefers-color-scheme: dark)")
    .addEventListener("change", () => paint());
  try {
    const remembered = localStorage.getItem(STORAGE_KEY);
    if (remembered) {
      shown = JSON.parse(remembered) as Appearance;
      paint();
    }
  } catch {
    shown = null;
  }
}
