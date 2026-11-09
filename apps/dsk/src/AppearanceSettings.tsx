import { useEffect, useRef, useState } from "react";
import {
  type AppearanceField,
  type AppearanceValue,
  formatValue,
  type InstalledFont,
  type Option,
  optionsOf,
  stepped,
  type Theme,
} from "./appearance";
import { errorText } from "./engine";
import {
  act,
  type ActionId,
  loadInstalledFonts,
  resetAppearance,
  saveAppearance,
  tryAppearance,
  useSettings,
} from "./keybinds";
import { inPane } from "./keys";
import { hint, SETTINGS_MOVES, useHints } from "./shortcuts";
import { Choice, Picker } from "./ui/Picker";
import { listMoves, SettingsPage } from "./ui/SettingsPage";
import { Icon } from "./ui/Icon";
import { Keys } from "./ui/StatusIcon";

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

/** How the app looks. The rows come from the settings crate, so one added there shows up here.
 * What can be done to the selected row stays in the bar at the bottom. */
export function AppearanceSettings({
  active,
  onActivate,
  onLeave,
}: {
  /** This list has the keyboard, rather than the sections beside it. */
  active: boolean;
  onActivate: () => void;
  onLeave: () => void;
}) {
  const settings = useSettings();
  const appearance = settings?.appearance;
  const rows = appearance?.fields ?? [];
  const fonts = useInstalledFonts();
  const [cursor, setCursor] = useState(0);
  const [picking, setPicking] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Choosing keeps the preview up until what was chosen arrives, so it doesn't flash back.
  const saving = useRef(false);
  const selected = Math.max(Math.min(cursor, rows.length - 1), 0);
  const row: AppearanceField | undefined = rows[selected];

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

  const change = (field: AppearanceField) => {
    if (field.kind.type === "toggle") step(field, 1);
    else if (field.kind.type !== "number") setPicking(field.id);
  };

  const choose = (field: AppearanceField, value: string) => {
    saving.current = true;
    void save(field.id, value).finally(() => {
      saving.current = false;
      tryAppearance(null);
    });
  };

  const onPickerOpen = (field: AppearanceField, index: number, open: boolean) => {
    setCursor(index);
    setPicking(open ? field.id : null);
    if (open) return;
    // The list says it's closing and what was chosen in one go, in an order that's its own.
    setTimeout(() => {
      if (!saving.current) tryAppearance(null);
    });
  };

  const isNumber = row?.kind.type === "number";
  useHints(
    "pane",
    active && row
      ? [
          ...(isNumber
            ? []
            : [hint(row.kind.type === "toggle" ? "Switch" : "Choose", "settings.edit")]),
          hint(isNumber ? "Lower, raise" : "Previous, next", "settings.decrease", "settings.increase"),
          ...(row.customized ? [hint("Reset", "settings.reset")] : []),
          ...SETTINGS_MOVES,
        ]
      : null,
  );

  // Leaving the screen with something still being tried out mustn't leave it on.
  useEffect(() => () => tryAppearance(null), []);

  useEffect(() => {
    if (active) document.querySelector(".keybind.current")?.scrollIntoView({ block: "nearest" });
  }, [selected, active]);

  useEffect(() => {
    if (!active || !row) return;
    const onKey = (event: KeyboardEvent) => {
      if (!inPane(event)) return;
      const commands: [ActionId, () => void][] = [
        ...listMoves(selected, rows.length, setCursor, onLeave),
        ["settings.edit", () => change(row)],
        ["settings.decrease", () => step(row, -1)],
        ["settings.increase", () => step(row, 1)],
      ];
      if (row.customized) commands.push(["settings.reset", () => reset(row)]);
      commands.some(([id, run]) => act(event, id, run));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const footer =
    active && row ? (
      <>
        <span className="settings-actions-what">{row.about || row.label}</span>
        {row.kind.type === "number" ? (
          <span className="settings-actions-what settings-actions-keys">
            <Keys action="settings.decrease" />
            <Keys action="settings.increase" />
            Change
          </span>
        ) : (
          <button className="button" onClick={() => change(row)}>
            {row.kind.type === "toggle" ? "Switch" : "Choose"}
            <Keys action="settings.edit" />
          </button>
        )}
        <button className="button" disabled={!row.customized} onClick={() => reset(row)}>
          Reset
          <Keys action="settings.reset" />
        </button>
      </>
    ) : (
      <span className="settings-actions-what">
        <Keys action="sections.confirm" /> Change these settings
      </span>
    );

  return (
    <>
      <Preview />
      <SettingsPage error={error} footer={footer}>
        {appearance?.groups.map((group) => (
          <section key={group.id} className="settings-group">
            <h3 className="settings-group-title">{group.title}</h3>
            <div className="keybinds">
              {rows
                .filter((field) => field.group === group.id)
                .map((field) => {
                  const index = rows.indexOf(field);
                  const current = active && index === selected;
                  return (
                    <div
                      key={field.id}
                      className={`keybind ${current ? "current" : ""}`}
                      // Moving, rather than entering: a row scrolling under a pointer that
                      // isn't moving must not take the keyboard or the selection.
                      onMouseMove={(event) => {
                        if (event.movementX === 0 && event.movementY === 0) return;
                        setCursor(index);
                        onActivate();
                      }}
                    >
                      <span className="name">{field.label}</span>
                      {field.customized && <span className="meta">Changed</span>}
                      <Control
                        field={field}
                        options={options(field)}
                        current={current}
                        open={current && picking === field.id}
                        onOpenChange={(open) => onPickerOpen(field, index, open)}
                        onChoose={(value) => choose(field, value)}
                        onStep={(direction) => step(field, direction)}
                      />
                    </div>
                  );
                })}
            </div>
          </section>
        ))}
        <p className="settings-note settings-group">
          You can write your own themes in the settings file. dsk.defaults.toml, next to it, shows
          how and lists every color.
        </p>
      </SettingsPage>
    </>
  );
}

function Control({
  field,
  options,
  current,
  open,
  onOpenChange,
  onChoose,
  onStep,
}: {
  field: AppearanceField;
  options: Option[];
  current: boolean;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onChoose: (value: string) => void;
  onStep: (direction: -1 | 1) => void;
}) {
  const { kind, label, id, value } = field;
  // Focus stays off these, so Enter does what the settings say rather than pressing them.
  const keepFocus = (event: React.MouseEvent) => event.preventDefault();

  if (kind.type === "number") {
    return (
      <span className="stepper">
        <button
          className="stepper-button"
          tabIndex={-1}
          aria-label={`Lower ${label}`}
          disabled={Number(value) <= kind.min}
          onMouseDown={keepFocus}
          onClick={() => onStep(-1)}
        >
          <Icon name="lower" />
        </button>
        <span className="stepper-value">{formatValue(field)}</span>
        <button
          className="stepper-button"
          tabIndex={-1}
          aria-label={`Raise ${label}`}
          disabled={Number(value) >= kind.max}
          onMouseDown={keepFocus}
          onClick={() => onStep(1)}
        >
          <Icon name="raise" />
        </button>
      </span>
    );
  }

  if (kind.type === "toggle") {
    return (
      <button
        className="picker"
        tabIndex={-1}
        role="switch"
        aria-checked={value === true}
        aria-label={label}
        onMouseDown={keepFocus}
        onClick={() => onStep(1)}
      >
        {formatValue(field)}
        {current && <Keys action="settings.edit" />}
      </button>
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
      finalFocus={false}
      trigger={
        <>
          {chosen?.theme && <ThemeSwatch theme={chosen.theme} />}
          {chosen?.label ?? String(value)}
          {current && <Keys action="settings.edit" />}
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
