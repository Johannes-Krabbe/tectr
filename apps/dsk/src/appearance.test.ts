import assert from "node:assert/strict";
import { test } from "node:test";
import {
  type Appearance,
  type AppearanceField,
  cssVariables,
  fontStack,
  formatValue,
  optionsOf,
  pageOf,
  previewedTheme,
  schemeOf,
  stepped,
  type Theme,
} from "./appearance.ts";

const number = (id: string, value: number, min: number, max: number, step: number, unit = "") =>
  ({
    id,
    group: "text",
    label: id,
    kind: { type: "number", min, max, step, unit },
    value,
    customized: false,
  }) satisfies AppearanceField;

const text = (id: string, value: string) =>
  ({
    id,
    group: "theme",
    label: id,
    kind: { type: "theme", scheme: "dark" },
    value,
    customized: false,
  }) satisfies AppearanceField;

const theme = (id: string, scheme: "dark" | "light", base: string): Theme => ({
  id,
  name: id,
  scheme,
  custom: false,
  colors: { "surface.base": base, "text.on-accent": base },
});

const appearance = (mode: string, overrides: AppearanceField[] = []): Appearance => {
  const fields: AppearanceField[] = [
    text("appearance.mode", mode),
    text("appearance.dark_theme", "night"),
    text("appearance.light_theme", "day"),
    text("appearance.text.family", ""),
    number("appearance.text.size", 13, 9, 28, 0.5, "px"),
    number("appearance.text.line_height", 1.55, 1, 2.5, 0.05),
    text("appearance.text.smoothing", "antialiased"),
    text("appearance.code.family", ""),
    number("appearance.code.size", 12.5, 8, 28, 0.5, "px"),
    number("appearance.code.line_height", 1.55, 1, 2.5, 0.05),
    { ...text("appearance.code.ligatures", ""), kind: { type: "toggle" }, value: true },
    number("appearance.layout.density", 100, 70, 150, 5, "%"),
    number("appearance.layout.corners", 100, 0, 300, 25, "%"),
    number("appearance.layout.sidebar_width", 256, 160, 560, 8, "px"),
    number("appearance.layout.content_width", 800, 480, 2400, 40, "px"),
  ];
  return {
    pages: [],
    groups: [],
    fields: fields.map((field) => overrides.find((o) => o.id === field.id) ?? field),
    themes: [theme("night", "dark", "dark-base"), theme("day", "light", "light-base")],
  };
};

test("the mode decides the scheme, and system follows the system", () => {
  assert.equal(schemeOf(appearance("dark"), false), "dark");
  assert.equal(schemeOf(appearance("light"), true), "light");
  assert.equal(schemeOf(appearance("system"), true), "dark");
  assert.equal(schemeOf(appearance("system"), false), "light");
});

test("trying out a theme shows it whichever mode is on", () => {
  const preview = { id: "appearance.dark_theme", value: "night" };
  assert.equal(schemeOf(appearance("light"), false, preview), "dark");
  const mode = { id: "appearance.mode", value: "dark" };
  assert.equal(schemeOf(appearance("light"), false, mode), "dark");
});

test("the window follows a preview that changes the theme, and nothing else", () => {
  const shown = appearance("dark");
  assert.equal(previewedTheme(shown, false, null), null);
  assert.equal(previewedTheme(shown, false, { id: "appearance.text.size", value: 15 }), null);
  for (const id of ["appearance.dark_theme", "appearance.light_theme"]) {
    assert.equal(previewedTheme(shown, false, { id, value: "other" }), "other");
  }
  assert.equal(previewedTheme(shown, false, { id: "appearance.mode", value: "light" }), "day");
  assert.equal(previewedTheme(shown, true, { id: "appearance.mode", value: "system" }), "night");
});

test("colors come from the theme for the scheme, under the names the tokens use", () => {
  const dark = cssVariables(appearance("system"), "dark");
  assert.equal(dark["--color-surface-base"], "dark-base");
  assert.equal(dark["--color-text-on-accent"], "dark-base");
  assert.equal(cssVariables(appearance("system"), "light")["--color-surface-base"], "light-base");
});

test("a preview changes what's painted, and only that", () => {
  const variables = cssVariables(appearance("dark"), "dark", {
    id: "appearance.code.family",
    value: "Fira Code",
  });
  assert.equal(variables["--font-code"], '"Fira Code", var(--font-code-stack)');
  assert.equal(variables["--font-ui"], "var(--font-ui-stack)");
});

test("sizes and layout become CSS values", () => {
  const variables = cssVariables(
    appearance("dark", [
      number("appearance.text.size", 15, 9, 28, 0.5, "px"),
      number("appearance.layout.density", 85, 70, 150, 5, "%"),
      { ...text("appearance.code.ligatures", ""), kind: { type: "toggle" }, value: false },
    ]),
    "dark",
  );
  assert.equal(variables["--font-ui-size"], "15px");
  assert.equal(variables["--density"], "0.85");
  assert.equal(variables["--corners"], "1");
  assert.equal(variables["--layout-sidebar-width"], "256px");
  assert.equal(variables["--font-code-ligatures"], "none");
});

test("a font goes in front of the bundled ones, and CSS's own names stay unquoted", () => {
  assert.equal(fontStack("", "--s"), "var(--s)");
  assert.equal(fontStack("JetBrains Mono", "--s"), '"JetBrains Mono", var(--s)');
  assert.equal(fontStack("ui-monospace", "--s"), "ui-monospace, var(--s)");
});

test("numbers step along whole steps and stop at the ends", () => {
  const up = (field: AppearanceField) => stepped(field, 1, []);
  const down = (field: AppearanceField) => stepped(field, -1, []);
  assert.equal(up(number("a", 13, 9, 28, 0.5)), 13.5);
  assert.equal(down(number("a", 13, 9, 28, 0.5)), 12.5);
  assert.equal(up(number("a", 13.25, 9, 28, 0.5)), 13.5);
  assert.equal(down(number("a", 13.25, 9, 28, 0.5)), 13);
  assert.equal(up(number("a", 28, 9, 28, 0.5)), 28);
  assert.equal(down(number("a", 9, 9, 28, 0.5)), 9);
  // These don't come out exact in floating point.
  assert.equal(up(number("a", 1.55, 1, 2.5, 0.05)), 1.6);
  assert.equal(down(number("a", 1.55, 1, 2.5, 0.05)), 1.5);
});

test("choices step through their list and stop at the ends", () => {
  const field = text("appearance.dark_theme", "b");
  const options = [{ value: "a", label: "A" }, { value: "b", label: "B" }];
  assert.equal(stepped(field, -1, options), "a");
  assert.equal(stepped(field, 1, options), "b");
  const toggle: AppearanceField = { ...field, kind: { type: "toggle" }, value: true };
  assert.equal(stepped(toggle, 1, []), false);
});

test("a theme list holds that scheme's themes", () => {
  const themes = [theme("night", "dark", "x"), theme("day", "light", "x")];
  const options = optionsOf(text("appearance.dark_theme", "night"), themes, null);
  assert.deepEqual(options.map((option) => option.value), ["night"]);
});

test("a code font list has monospaced fonts first, and says when what's set isn't installed", () => {
  const field: AppearanceField = {
    ...text("appearance.code.family", "Gone Mono"),
    kind: { type: "font", bundled: "Geist Mono", system: "ui-monospace", monospace: true },
  };
  const fonts = [
    { name: "Arial", monospace: false },
    { name: "Menlo", monospace: true },
  ];
  const options = optionsOf(field, [], fonts);
  assert.deepEqual(
    options.map((option) => option.value),
    ["", "ui-monospace", "Gone Mono", "Menlo", "Arial"],
  );
  assert.equal(options[2].meta, "Not installed");
  // Before the fonts have been read, nothing is called missing.
  assert.equal(optionsOf(field, [], null).length, 2);
});

test("values are shown with their unit", () => {
  assert.equal(formatValue(number("a", 13.5, 9, 28, 0.5, "px")), "13.5px");
  assert.equal(formatValue(number("a", 100, 70, 150, 5, "%")), "100%");
  assert.equal(
    formatValue({ ...text("a", ""), kind: { type: "toggle" }, value: false }),
    "Off",
  );
});

test("a page shows its groups in order, and the cursor moves through rows the same way", () => {
  const groups = [
    { id: "interface", title: "Interface", page: "general" },
    { id: "text", title: "Text", page: "appearance" },
    { id: "motion", title: "Motion", page: "appearance" },
  ];
  // Not in the order the page shows them, which is group by group.
  const fields = [
    { ...number("appearance.text.size", 13, 9, 28, 0.5), group: "text" },
    { ...number("general.show_shortcuts", 0, 0, 1, 1), group: "interface" },
    { ...number("appearance.motion.status_icons", 1, 0, 1, 1), group: "motion" },
    { ...number("appearance.text.line_height", 1.55, 1, 2.5, 0.05), group: "text" },
    { ...number("appearance.motion.transcript", 1, 0, 1, 1), group: "motion" },
  ];
  const page = pageOf({ groups, fields }, "appearance");
  assert.deepEqual(
    page.groups.map((group) => group.id),
    ["text", "motion"],
  );
  assert.deepEqual(
    page.rows.map((row) => row.id),
    [
      "appearance.text.size",
      "appearance.text.line_height",
      "appearance.motion.status_icons",
      "appearance.motion.transcript",
    ],
  );
  assert.deepEqual(pageOf({ groups, fields }, "nowhere"), { groups: [], rows: [] });
});
