import { useEffect, useState } from "react";
import { AppearanceSettings } from "./AppearanceSettings";
import { act, type ActionId, useSettings } from "./keybinds";
import { inPane } from "./keys";
import { KeybindSettings } from "./KeybindSettings";
import { hint, useHints } from "./shortcuts";
import { SettingsLayout, SettingsPage } from "./ui/SettingsPage";

const SECTIONS = [
  { id: "general", label: "General" },
  { id: "appearance", label: "Appearance" },
  { id: "keybinds", label: "Keybinds" },
] as const;

type SectionId = (typeof SECTIONS)[number]["id"];

function GeneralSettings() {
  const settings = useSettings();
  return (
    <SettingsPage>
      {settings && (
        <section className="settings-group">
          <h3 className="settings-group-title">Settings file</h3>
          <p className="settings-file code">{settings.path}</p>
        </section>
      )}
    </SettingsPage>
  );
}

/** This app's settings, with the sections in the sidebar's place. Like the main sidebar, the
 * section list has the keyboard first, shows what it lands on, and you go into a section
 * that has something to move through. */
export function AppSettings({
  section: shown,
  onSection,
  onClose,
}: {
  section: string;
  onSection: (section: string) => void;
  onClose: () => void;
}) {
  const section: SectionId = SECTIONS.find((s) => s.id === shown)?.id ?? "general";
  // Whether the section has the keyboard, rather than the list of sections.
  const [inside, setInside] = useState(false);
  const canGoInside = section === "keybinds" || section === "appearance";
  const select = onSection;

  useHints(
    "pane",
    inside
      ? null
      : [
          ...(canGoInside ? [hint("Go in", "sections.confirm", "sections.open")] : []),
          hint("Section", "sections.up", "sections.down"),
          hint("First, last", "sections.first", "sections.last"),
          hint("Close", "sections.close"),
        ],
  );

  useEffect(() => {
    if (inside) return;
    const onKey = (event: KeyboardEvent) => {
      if (!inPane(event)) return;
      const index = SECTIONS.findIndex((s) => s.id === section);
      const moveBy = (step: number) => () => {
        const next = SECTIONS[index + step];
        if (next) select(next.id);
      };
      const goInside = () => {
        if (canGoInside) setInside(true);
      };
      const commands: [ActionId, () => void][] = [
        ["sections.up", moveBy(-1)],
        ["sections.down", moveBy(1)],
        ["sections.first", () => select(SECTIONS[0].id)],
        ["sections.last", () => select(SECTIONS[SECTIONS.length - 1].id)],
        ["sections.open", goInside],
        ["sections.confirm", goInside],
        ["sections.close", onClose],
      ];
      commands.some(([id, run]) => act(event, id, run));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  return (
    <SettingsLayout
      heading="App settings"
      sections={SECTIONS}
      section={section}
      active={!inside}
      closeKeys={inside ? undefined : "sections.close"}
      onSelect={(id) => {
        select(id);
        setInside(false);
      }}
      onClose={onClose}
    >
      {section === "general" && <GeneralSettings />}
      {section === "appearance" && (
        <AppearanceSettings
          active={inside}
          onActivate={() => setInside(true)}
          onLeave={() => setInside(false)}
        />
      )}
      {section === "keybinds" && (
        <KeybindSettings
          active={inside}
          onActivate={() => setInside(true)}
          onLeave={() => setInside(false)}
        />
      )}
    </SettingsLayout>
  );
}
