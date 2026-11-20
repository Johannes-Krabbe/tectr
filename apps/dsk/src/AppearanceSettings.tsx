import { type ReactNode, useEffect, useRef, useState } from "react";
import {
  type AppearanceField,
  type AppearanceValue,
  formatValue,
  type InstalledFont,
  type Option,
  optionsOf,
  pageOf,
  stepped,
  type Theme,
} from "./appearance";
import { errorText } from "./engine";
import { loadInstalledFonts, resetAppearance, saveAppearance, tryAppearance, useSettings } from "./keybinds";
import { Choice, Picker } from "./ui/Picker";
import { ResetButton, SettingRow, SettingsGroupBox, SettingsPage, Stepper, Switch } from "./ui/SettingsPage";

// Reading the fonts takes a moment and they rarely change, so once per run is enough.
let fontsOnce: Promise<InstalledFont[]> | null = null;

function useInstalledFonts(): InstalledFont[] | null {
  const [fonts, setFonts] = useState<InstalledFont[] | null>(null);
  useEffect(() => {
    let current = true;
    fontsOnce ??= loadInstalledFonts();
    fontsOnce.then((loaded) => {
      if (current) setFonts(loaded);
    });
    return () => {
      current = false;
    };
  }, []);
  return fonts;
}

const SWATCH: string[] = [
  "surface.panel",
  "text.primary",
  "accent.solid",
  "syntax.keyword",
  "syntax.string",
  "syntax.function",
];

/** A theme's own colors, so they can be told apart before choosing. */
function ThemeSwatch({ theme }: { theme: Theme }) {
  return (
    <span className="theme-swatch" aria-hidden="true">
      {SWATCH.map((token) => (
        <span key={token} style={{ background: theme.colors[token] }} />
      ))}
    </span>
  );
}

/** Settings has no agent to look at, so this shows what the text and code settings change. */
function Preview() {
  return (
    <div className="appearance-preview">
      <div className="appearance-preview-in agent-message">
        <p>
          Messages look like this, with <code>inline code</code>, <strong>bold text</strong> and{" "}
          <span className="md-link">a link</span>.
        </p>
        <pre>
          <code>
            <span className="syntax-comment">{"// Code looks like this"}</span>
            {"\n"}
            <span className="syntax-keyword">fn</span>{" "}
            <span className="syntax-function">greet</span>
            <span className="syntax-punctuation">(</span>
            <span className="syntax-plain">name</span>
            <span className="syntax-punctuation">:</span>{" "}
            <span className="syntax-type">String</span>
            <span className="syntax-punctuation">{") -> "}</span>
            <span className="syntax-type">usize</span>{" "}
            <span className="syntax-punctuation">{"{"}</span>
            {"\n    "}
            <span className="syntax-function">print</span>
            <span className="syntax-punctuation">(</span>
            <span className="syntax-string">"hello"</span>
            <span className="syntax-punctuation">{");"}</span>{" "}
            <span className="syntax-constant">42</span>
            {"\n"}
            <span className="syntax-punctuation">{"}"}</span>
          </code>
        </pre>
      </div>
    </div>
  );
}

/** One page of the app's settings. The rows come from the settings crate, so one added there
 * shows up here. A row is its label and its control, used right where it is, and a reset button
 * when it differs from the default. `children` is what the page has besides rows. */
export function AppearanceSettings({ page, children }: { page: string; children?: ReactNode }) {
  const settings = useSettings();
  const appearance = settings?.appearance;
  const { groups, rows } = appearance ? pageOf(appearance, page) : { groups: [], rows: [] };
  const fonts = useInstalledFonts();
  const [picking, setPicking] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Choosing keeps the preview up until what was chosen arrives, so it doesn't flash back.
  const saving = useRef(false);

  const options = (field: AppearanceField): Option[] =>
    optionsOf(field, appearance?.themes ?? [], fonts);

  const save = (id: string, value: AppearanceValue) => {
    setError(null);
    return saveAppearance(id, value).catch((reason) => setError(errorText(reason)));
  };

  const step = (field: AppearanceField, direction: -1 | 1) => {
    const next = stepped(field, direction, options(field));
    if (next !== field.value) void save(field.id, next);
  };

  const reset = (field: AppearanceField) => {
    setError(null);
    resetAppearance(field.id).catch((reason) => setError(errorText(reason)));
  };

  const choose = (field: AppearanceField, value: string) => {
    saving.current = true;
    void save(field.id, value).finally(() => {
      saving.current = false;
      tryAppearance(null);
    });
  };

  const onPickerOpen = (field: AppearanceField, open: boolean) => {
    setPicking(open ? field.id : null);
    if (open) return;
    // The list says it's closing and what was chosen in one go, in an order that's its own.
    setTimeout(() => {
      if (!saving.current) tryAppearance(null);
    });
  };

  // Leaving the screen with something still being tried out mustn't leave it on.
  useEffect(() => () => tryAppearance(null), []);

  return (
    <>
      {page === "appearance" && <Preview />}
      <SettingsPage error={error}>
        {groups.map((group) => (
          <SettingsGroupBox key={group.id} title={group.title}>
            {rows
              .filter((field) => field.group === group.id)
              .map((field) => (
                <SettingRow
                  key={field.id}
                  label={field.label}
                  onStep={(direction) => step(field, direction)}
                >
                  {field.customized && <ResetButton label={field.label} onReset={() => reset(field)} />}
                  <Control
                    field={field}
                    options={options(field)}
                    open={picking === field.id}
                    onOpenChange={(open) => onPickerOpen(field, open)}
                    onChoose={(value) => choose(field, value)}
                    onStep={(direction) => step(field, direction)}
                  />
                </SettingRow>
              ))}
          </SettingsGroupBox>
        ))}
        {children}
      </SettingsPage>
    </>
  );
}

function Control({
  field,
  options,
  open,
  onOpenChange,
  onChoose,
  onStep,
}: {
  field: AppearanceField;
  options: Option[];
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onChoose: (value: string) => void;
  onStep: (direction: -1 | 1) => void;
}) {
  const { kind, label, id, value } = field;

  if (kind.type === "number") {
    return (
      <Stepper
        label={label}
        value={Number(value)}
        shown={formatValue(field)}
        min={kind.min}
        max={kind.max}
        onStep={onStep}
      />
    );
  }

  if (kind.type === "toggle") {
    return (
      <Switch label={label} checked={value === true} onChange={(checked) => onStep(checked ? 1 : -1)} />
    );
  }

  const chosen = options.find((option) => option.value === value);
  return (
    <Picker
      label={label}
      className="picker"
      value={String(value)}
      open={open}
      onOpenChange={onOpenChange}
      onChange={onChoose}
      trigger={
        <>
          {chosen?.theme && <ThemeSwatch theme={chosen.theme} />}
          {chosen?.label ?? String(value)}
        </>
      }
    >
      {options.map((option) => (
        <Choice
          key={option.value}
          value={option.value}
          label={option.label}
          meta={option.meta}
          leading={option.theme && <ThemeSwatch theme={option.theme} />}
          onHighlight={() => tryAppearance({ id, value: option.value })}
        />
      ))}
    </Picker>
  );
}
