import { useEffect, useState } from "react";
import { errorText } from "./engine";
import {
  act,
  type ActionId,
  type Binding,
  bindingLabel,
  type KeyAction,
  MAC,
  matches,
  pauseKeys,
  recordedChord,
  resetBindings,
  saveBindings,
  sequenceTimeout,
  useSettings,
} from "./keybinds";
import { formatSequence } from "./keymatch";
import { inPane } from "./keys";
import { hint, SETTINGS_MOVES, useHints } from "./shortcuts";
import { GoInHint, listMoves, SettingsPage } from "./ui/SettingsPage";
import { Kbd, Keys } from "./ui/StatusIcon";

/** What switching holding on starts with. The file can set any other length. */
const DEFAULT_HOLD_MS = 500;

interface Recording {
  mode: "replace" | "add";
  /** Pressed but not saved yet. Any key can be a shortcut, including the ones that save
   * and cancel, so those only mean that once something has been pressed. */
  chord: string | null;
  /** When the last key was pressed. Another one soon after makes it a sequence. */
  at: number;
}

const MAX_SEQUENCE = 4;

/** Every shortcut in the app. What can be done to the selected one stays in a bar at the
 * bottom, so moving through the list never moves the list. */
export function KeybindSettings({
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
  const rows = settings?.keybinds ?? [];
  const [cursor, setCursor] = useState(0);
  const [recording, setRecording] = useState<Recording | null>(null);
  const [error, setError] = useState<string | null>(null);
  const selected = Math.max(Math.min(cursor, rows.length - 1), 0);
  const row: KeyAction | undefined = rows[selected];
  const hasKeys = (row?.bindings.length ?? 0) > 0;

  const update = (id: ActionId, bindings: Binding[]) => {
    setError(null);
    saveBindings(id, bindings).catch((reason) => setError(errorText(reason)));
  };

  const reset = (id: ActionId) => {
    setError(null);
    resetBindings(id).catch((reason) => setError(errorText(reason)));
  };

  const toggleHold = (action: KeyAction) => {
    const on = action.bindings.some((binding) => binding.hold_ms > 0);
    update(
      action.id,
      action.bindings.map((binding) => ({ ...binding, hold_ms: on ? 0 : DEFAULT_HOLD_MS })),
    );
  };

  const toggleRepeat = (action: KeyAction) => {
    const on = action.bindings.some((binding) => binding.repeat);
    update(
      action.id,
      action.bindings.map((binding) => ({ ...binding, repeat: !on })),
    );
  };

  const commit = (action: KeyAction, { mode, chord }: Recording) => {
    if (!chord) return;
    const like = action.bindings[0];
    const binding: Binding = {
      key: chord,
      hold_ms: like?.hold_ms ?? 0,
      repeat: like?.repeat ?? action.repeat,
    };
    update(
      action.id,
      mode === "add"
        ? [...action.bindings.filter((other) => other.key !== chord), binding]
        : [binding],
    );
    setRecording(null);
  };

  useHints(
    "pane",
    active && !recording && row
      ? [
          hint("Change", "settings.edit"),
          hint("Add key", "settings.add"),
          ...(hasKeys
            ? [
                hint("Remove", "settings.unbind"),
                hint("Holding", "settings.toggle_hold"),
                hint("Repeating", "settings.toggle_repeat"),
              ]
            : []),
          ...(row.customized ? [hint("Reset", "settings.reset")] : []),
          ...SETTINGS_MOVES,
        ]
      : null,
  );
  // Until something is pressed every key is the new shortcut, including the ones that save.
  useHints(
    "overlay",
    recording
      ? recording.chord
        ? [
            hint("Save", "recorder.save"),
            hint("Cancel", "recorder.cancel"),
            hint("Keep pressing to make a sequence"),
          ]
        : [hint("Press the new shortcut")]
      : null,
  );

  // Going back to the sections with the pointer ends a recording, which needs a selected row.
  useEffect(() => {
    if (!active) setRecording(null);
  }, [active]);

  useEffect(() => {
    if (active) document.querySelector(".keybind.current")?.scrollIntoView({ block: "nearest" });
  }, [selected, active]);

  useEffect(() => {
    if (!active || recording || !row) return;
    const onKey = (event: KeyboardEvent) => {
      if (!inPane(event)) return;
      const commands: [ActionId, () => void][] = [
        ...listMoves(selected, rows.length, setCursor, onLeave),
        ["settings.edit", () => setRecording({ mode: "replace", chord: null, at: 0 })],
        ["settings.add", () => setRecording({ mode: "add", chord: null, at: 0 })],
      ];
      if (hasKeys) {
        commands.push(
          ["settings.unbind", () => update(row.id, [])],
          ["settings.toggle_hold", () => toggleHold(row)],
          ["settings.toggle_repeat", () => toggleRepeat(row)],
        );
      }
      if (row.customized) commands.push(["settings.reset", () => reset(row.id)]);
      commands.some(([id, run]) => act(event, id, run));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  // Takes every key while recording, ahead of everything else in the app.
  useEffect(() => {
    if (!recording || !row) return;
    pauseKeys(true);
    const onKey = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopImmediatePropagation();
      if (event.repeat) return;
      if (recording.chord && matches(event, "recorder.save")) commit(row, recording);
      else if (recording.chord && matches(event, "recorder.cancel")) setRecording(null);
      else {
        const chord = recordedChord(event);
        if (!chord) return;
        const at = Date.now();
        const continues =
          recording.chord !== null &&
          at - recording.at < sequenceTimeout() &&
          recording.chord.split(" ").length < MAX_SEQUENCE;
        setRecording({ ...recording, chord: continues ? `${recording.chord} ${chord}` : chord, at });
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      pauseKeys(false);
    };
  }, [recording, row]);

  const footer = recording ? (
    <>
      <span className="settings-actions-what">
        {recording.chord
          ? "Keep pressing to make a sequence like gg"
          : `Press the new shortcut for ${row?.label}`}
      </span>
      <button
        className="button primary"
        disabled={!recording.chord}
        onClick={() => row && commit(row, recording)}
      >
        Save
        {recording.chord && <Keys action="recorder.save" />}
      </button>
      <button className="button" onClick={() => setRecording(null)}>
        Cancel
        {recording.chord && <Keys action="recorder.cancel" />}
      </button>
    </>
  ) : active && row ? (
    <>
      <span className="settings-actions-what" />
      <button className="button" onClick={() => setRecording({ mode: "replace", chord: null, at: 0 })}>
        Change
        <Keys action="settings.edit" />
      </button>
      <button className="button" onClick={() => setRecording({ mode: "add", chord: null, at: 0 })}>
        Add
        <Keys action="settings.add" />
      </button>
      <button className="button" disabled={!hasKeys} onClick={() => update(row.id, [])}>
        Remove
        <Keys action="settings.unbind" />
      </button>
      <button className="button" disabled={!hasKeys} onClick={() => toggleHold(row)}>
        {row.bindings.some((binding) => binding.hold_ms > 0) ? "Holding on" : "Holding off"}
        <Keys action="settings.toggle_hold" />
      </button>
      <button className="button" disabled={!hasKeys} onClick={() => toggleRepeat(row)}>
        {row.bindings.some((binding) => binding.repeat) ? "Repeating on" : "Repeating off"}
        <Keys action="settings.toggle_repeat" />
      </button>
      <button className="button" disabled={!row.customized} onClick={() => reset(row.id)}>
        Reset
        <Keys action="settings.reset" />
      </button>
    </>
  ) : (
    <GoInHint />
  );

  return (
    <SettingsPage error={error} footer={footer}>
      {settings?.groups.map((group) => (
        <section key={group.id} className="settings-group">
          <h3 className="settings-group-title">{group.title}</h3>
          <div className="keybinds">
            {rows
              .filter((action) => action.group === group.id)
              .map((action) => {
                const index = rows.indexOf(action);
                const current = active && index === selected;
                const recordingHere = current ? recording : null;
                return (
                  <div
                    key={action.id}
                    className={`keybind ${current ? "current" : ""}`}
                    // Moving, rather than entering: a row scrolling under a pointer that
                    // isn't moving must not take the keyboard or the selection.
                    onMouseMove={(event) => {
                      if (recording || (event.movementX === 0 && event.movementY === 0)) return;
                      setCursor(index);
                      onActivate();
                    }}
                  >
                    <span className="name">{action.label}</span>
                    {!recordingHere && action.customized && <span className="meta">Changed</span>}
                    {current && !recording && !action.customized && action.inherits && (
                      <span className="meta">Follows {action.inherits}</span>
                    )}
                    {recordingHere ? (
                      recordingHere.chord ? (
                        <Kbd>{formatSequence(recordingHere.chord, MAC)}</Kbd>
                      ) : (
                        <span className="meta">Press a key</span>
                      )
                    ) : action.bindings.length > 0 ? (
                      action.bindings.map((binding) => (
                        <Kbd key={binding.key}>{bindingLabel(binding)}</Kbd>
                      ))
                    ) : (
                      <span className="meta">No key</span>
                    )}
                  </div>
                );
              })}
          </div>
        </section>
      ))}
    </SettingsPage>
  );
}
