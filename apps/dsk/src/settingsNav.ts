// The one list of sections every settings screen shares: the app's own, then each device's.
// Which section is open is part of the view, so moving through the list is just showing views.

import type { View } from "./navigation";

export type SettingsView = Extract<View, { kind: "app_settings" | "device_settings" }>;

export interface Section {
  id: string;
  label: string;
}

export interface SettingsEntry {
  label: string;
  view: SettingsView;
}

/** Whose settings a run of sections are. */
export interface SettingsGroup {
  /** "app", or the device's machine ID. */
  id: string;
  heading: string;
  entries: SettingsEntry[];
}

export const DEVICE_SECTIONS = [
  { id: "general", label: "General" },
  { id: "providers", label: "Providers" },
  { id: "mcp", label: "MCP servers" },
  { id: "pairing", label: "Pairing" },
] as const;

export type DeviceSectionId = (typeof DEVICE_SECTIONS)[number]["id"];

/** The app's sections: a page for each kind of setting, which the settings crate lists, then
 * the two with screens of their own. */
export function appSections(pages: { id: string; title: string }[]): Section[] {
  return [
    ...pages.map((page) => ({ id: page.id, label: page.title })),
    { id: "keybinds", label: "Keybinds" },
    { id: "devices", label: "Devices" },
  ];
}

/** The section a view asks for, or the first when it asks for none or for one that's gone. */
export function sectionOf<S extends Section>(sections: readonly S[], shown: string | undefined): S {
  return sections.find((section) => section.id === shown) ?? sections[0];
}

export function settingsGroups(
  app: Section[],
  devices: { machineId: string; name: string }[],
): SettingsGroup[] {
  return [
    {
      id: "app",
      heading: "This app",
      entries: app.map(
        ({ id, label }): SettingsEntry => ({ label, view: { kind: "app_settings", section: id } }),
      ),
    },
    ...devices.map(({ machineId, name }) => ({
      id: machineId,
      heading: name,
      entries: DEVICE_SECTIONS.map(
        ({ id, label }): SettingsEntry => ({
          label,
          view: { kind: "device_settings", machineId, section: id },
        }),
      ),
    })),
  ];
}

const sameSettings = (a: SettingsView, b: SettingsView) =>
  a.kind === "device_settings"
    ? b.kind === "device_settings" && a.machineId === b.machineId
    : b.kind === "app_settings";

/** Where a view is in the list, or -1 when its settings aren't listed. A section that isn't
 * there is shown as the first of its settings, so that's where it is. */
export function entryIndex(entries: SettingsEntry[], view: SettingsView): number {
  const exact = entries.findIndex(
    (entry) => sameSettings(entry.view, view) && entry.view.section === view.section,
  );
  return exact !== -1 ? exact : entries.findIndex((entry) => sameSettings(entry.view, view));
}
