import assert from "node:assert/strict";
import { test } from "node:test";
import {
  appSections,
  DEVICE_SECTIONS,
  entryIndex,
  sectionOf,
  settingsGroups,
} from "./settingsNav.ts";

const pages = [
  { id: "general", title: "General" },
  { id: "appearance", title: "Appearance" },
];
const app = appSections(pages);
const groups = settingsGroups(app, [
  { machineId: "m1", name: "Laptop" },
  { machineId: "m2", name: "VPS" },
]);
const entries = groups.flatMap((group) => group.entries);

test("the app's sections are its pages, then keybinds and devices", () => {
  assert.deepEqual(
    app.map((section) => section.id),
    ["general", "appearance", "keybinds", "devices"],
  );
});

test("one list holds the app's sections and then every device's", () => {
  assert.deepEqual(
    groups.map((group) => group.heading),
    ["This app", "Laptop", "VPS"],
  );
  assert.equal(entries.length, app.length + 2 * DEVICE_SECTIONS.length);
  assert.deepEqual(groups[1].entries[2], {
    label: "MCP servers",
    view: { kind: "device_settings", machineId: "m1", section: "mcp" },
  });
});

test("moving down from the app's last section lands on the first device's first", () => {
  const last = entryIndex(entries, { kind: "app_settings", section: "devices" });
  assert.equal(last, app.length - 1);
  assert.deepEqual(entries[last + 1].view, {
    kind: "device_settings",
    machineId: "m1",
    section: "general",
  });
});

test("a device's settings are found by device and section", () => {
  const index = entryIndex(entries, { kind: "device_settings", machineId: "m2", section: "pairing" });
  assert.equal(index, entries.length - 1);
});

test("a view without a section, or with one that's gone, is at the first of its settings", () => {
  assert.equal(entryIndex(entries, { kind: "device_settings", machineId: "m2" }), app.length + 4);
  assert.equal(entryIndex(entries, { kind: "app_settings", section: "gone" }), 0);
  assert.equal(sectionOf(app, "gone").id, "general");
  assert.equal(sectionOf(app, undefined).id, "general");
  assert.equal(sectionOf(DEVICE_SECTIONS, "mcp").id, "mcp");
});

test("a device that isn't listed has no place in the list", () => {
  assert.equal(entryIndex(entries, { kind: "device_settings", machineId: "elsewhere" }), -1);
});
