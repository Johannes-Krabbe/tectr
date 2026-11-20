import { AppearanceSettings } from "./AppearanceSettings";
import { DevicesSettings } from "./DevicesSettings";
import type { Device } from "./engine";
import { useSettings } from "./keybinds";
import { KeybindSettings } from "./KeybindSettings";
import { appSections, sectionOf } from "./settingsNav";
import { SettingsGroupBox, SettingsLayout, type SettingsNav } from "./ui/SettingsPage";

/** This app's settings. Most sections are a page of rows from the settings crate, sorted by
 * what they're about. Keybinds and devices have screens of their own. */
export function AppSettings({
  section: shown,
  nav,
  devices,
  onAddDevice,
  onPair,
  onForget,
  onOpenDevice,
}: {
  section: string;
  nav: SettingsNav;
  devices: Device[];
  onAddDevice: () => void;
  onPair: (() => void) | undefined;
  onForget: (device: Device) => void;
  onOpenDevice: (device: Device) => void;
}) {
  const settings = useSettings();
  // Until the settings arrive there are no pages to choose between, and nothing to show.
  const section = settings ? sectionOf(appSections(settings.appearance.pages), shown).id : shown;
  return (
    <SettingsLayout nav={nav} current={{ kind: "app_settings", section }}>
      {section === "keybinds" ? (
        <KeybindSettings />
      ) : section === "devices" ? (
        <DevicesSettings
          devices={devices}
          onAddDevice={onAddDevice}
          onPair={onPair}
          onForget={onForget}
          onOpen={onOpenDevice}
        />
      ) : (
        // A page keeps its own cursor, so each one starts at its top.
        <AppearanceSettings key={section} page={section}>
          {section === "general" && settings && (
            <SettingsGroupBox title="Settings file">
              <p className="settings-file code">{settings.path}</p>
            </SettingsGroupBox>
          )}
        </AppearanceSettings>
      )}
    </SettingsLayout>
  );
}
