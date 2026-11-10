import { useState } from "react";
import { AppearanceSettings } from "./AppearanceSettings";
import { DevicesSettings } from "./DevicesSettings";
import type { Device } from "./engine";
import { useSettings } from "./keybinds";
import { KeybindSettings } from "./KeybindSettings";
import { appSections, sectionOf } from "./settingsNav";
import { SettingsLayout, type SettingsNav } from "./ui/SettingsPage";

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
  // Whether the section has the keyboard, rather than the list of sections.
  const [inside, setInside] = useState(false);
  const list = {
    active: inside,
    onActivate: () => setInside(true),
    onLeave: () => setInside(false),
  };

  return (
    <SettingsLayout
      nav={nav}
      current={{ kind: "app_settings", section }}
      inside={inside}
      onInside={setInside}
    >
      {section === "keybinds" ? (
        <KeybindSettings {...list} />
      ) : section === "devices" ? (
        <DevicesSettings
          {...list}
          devices={devices}
          onAddDevice={onAddDevice}
          onPair={onPair}
          onForget={onForget}
          onOpen={onOpenDevice}
        />
      ) : (
        // A page keeps its own cursor, so each one starts at its top.
        <AppearanceSettings key={section} page={section} {...list}>
          {section === "general" && settings && (
            <section className="settings-group">
              <h3 className="settings-group-title">Settings file</h3>
              <p className="settings-file code">{settings.path}</p>
            </section>
          )}
        </AppearanceSettings>
      )}
    </SettingsLayout>
  );
}
