import { useEffect, useRef, useState } from "react";
import { errorText } from "./engine";
import {
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
  titled,
  useSettings,
} from "./keybinds";
import { formatSequence } from "./keymatch";
import { Icon } from "./ui/Icon";
import { MenuButton, type MenuGroups } from "./ui/Menu";
import { ResetButton, SettingRow, SettingsGroupBox, SettingsPage } from "./ui/SettingsPage";

/** What switching holding on starts with. The file can set any other length. */
const DEFAULT_HOLD_MS = 500;

const MAX_SEQUENCE = 4;

interface Recording {
  id: ActionId;
  /** The key being replaced, or a new one. */
  slot: number | "add";
  /** Pressed but not saved yet. Any key can be a shortcut, including the ones that save
   * and cancel, so those only mean that once something has been pressed. */
  chord: string | null;
  /** When the last key was pressed. Another one soon after makes it a sequence. */
  at: number;
}

/** Every word typed has to appear in the action's label, its group or one of its keys. */
function matchesQuery(action: KeyAction, group: string, query: string): boolean {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return true;
  const text = [action.label, group, ...action.bindings.map(bindingLabel)].join(" ").toLowerCase();
  return words.every((word) => text.includes(word));
}

/** Every shortcut in the app, as a form: a row per action with its keys as buttons. Clicking a
 * key records a new one in its place, and the row's menu adds, removes and changes how a key
 * has to be pressed. A search box finds an action among the many. */
export function KeybindSettings() {
  const settings = useSettings();
  const rows = settings?.keybinds ?? [];
  const [query, setQuery] = useState("");
  const [recording, setRecording] = useState<Recording | null>(null);
  const [error, setError] = useState<string | null>(null);
  // The chip and buttons of the recording, so a click anywhere else can end it.
  const recorder = useRef<HTMLSpanElement>(null);
  const recordingAction = recording && rows.find((action) => action.id === recording.id);

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

  const record = (id: ActionId, slot: number | "add") => setRecording({ id, slot, chord: null, at: 0 });

  const commit = (action: KeyAction, { slot, chord }: Recording) => {
    if (!chord) return;
    // A new key is pressed the way the one it replaces was, or else like the action's others.
    const like = (slot === "add" ? undefined : action.bindings[slot]) ?? action.bindings[0];
    const binding: Binding = {
      key: chord,
      hold_ms: like?.hold_ms ?? 0,
      repeat: like?.repeat ?? action.repeat,
    };
    const others = action.bindings.filter((other, index) => other.key !== chord && index !== slot);
    const next =
      slot === "add"
        ? [...others, binding]
        : action.bindings.flatMap((other, index) =>
            index === slot ? [binding] : others.includes(other) ? [other] : [],
          );
    update(action.id, next);
    setRecording(null);
  };

  // Takes every key while recording, ahead of everything else in the app.
  useEffect(() => {
    if (!recording || !recordingAction) return;
    pauseKeys(true);
    const onKey = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopImmediatePropagation();
      if (event.repeat) return;
      if (recording.chord && matches(event, "recorder.save")) commit(recordingAction, recording);
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
    // Pointing somewhere else is leaving it, the way it is for a text box.
    const onPointer = (event: MouseEvent) => {
      if (!(event.target instanceof Node) || !recorder.current?.contains(event.target)) setRecording(null);
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("mousedown", onPointer, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("mousedown", onPointer, true);
      pauseKeys(false);
    };
  }, [recording, recordingAction]);

  const menu = (action: KeyAction): MenuGroups => {
    const hasKeys = action.bindings.length > 0;
    const keys = [
      { id: "add", label: "Add another key", run: () => record(action.id, "add") },
      ...action.bindings.map((binding, index) => ({
        id: `remove/${binding.key}`,
        label: `Remove ${bindingLabel(binding)}`,
        run: () => update(action.id, action.bindings.filter((_, other) => other !== index)),
      })),
    ];
    if (!hasKeys) return [keys];
    const holds = action.bindings.some((binding) => binding.hold_ms > 0);
    const repeats = action.bindings.some((binding) => binding.repeat);
    return [
      keys,
      [
        {
          id: "hold",
          label: holds ? "Don't require holding" : "Hold to press",
          run: () => toggleHold(action),
        },
        {
          id: "repeat",
          label: repeats ? "Don't repeat" : "Repeat while held",
          run: () => toggleRepeat(action),
        },
      ],
    ];
  };

  const recorderFor = (action: KeyAction, current: Recording) => (
    <span ref={recorder} className="key-recording">
      <span className="kbd key-chip recording">
        {current.chord ? formatSequence(current.chord, MAC) : "Press a key"}
      </span>
      <button
        className="button small primary"
        disabled={!current.chord}
        title={titled("Save", "recorder.save")}
        onClick={() => commit(action, current)}
      >
        Save
      </button>
      <button className="button small" title={titled("Cancel", "recorder.cancel")} onClick={() => setRecording(null)}>
        Cancel
      </button>
    </span>
  );

  const keysOf = (action: KeyAction) => {
    const current = recording?.id === action.id ? recording : null;
    const chips = action.bindings.map((binding, index) =>
      current?.slot === index ? (
        <span key={binding.key}>{recorderFor(action, current)}</span>
      ) : (
        <button
          key={binding.key}
          className="kbd key-chip"
          title={`Change ${bindingLabel(binding)}`}
          onClick={() => record(action.id, index)}
        >
          {bindingLabel(binding)}
        </button>
      ),
    );
    if (current?.slot === "add") chips.push(<span key="add">{recorderFor(action, current)}</span>);
    else if (action.bindings.length === 0) {
      chips.push(
        <button key="none" className="kbd key-chip empty" title="Choose a key" onClick={() => record(action.id, "add")}>
          No key
        </button>,
      );
    }
    return chips;
  };

  const groups = (settings?.groups ?? [])
    .map((group) => ({
      group,
      actions: rows.filter((action) => action.group === group.id && matchesQuery(action, group.title, query)),
    }))
    .filter(({ actions }) => actions.length > 0);

  return (
    <SettingsPage error={error}>
      <input
        className="settings-search"
        type="search"
        placeholder="Search shortcuts"
        aria-label="Search shortcuts"
        value={query}
        spellCheck={false}
        autoCorrect="off"
        autoCapitalize="off"
        onChange={(event) => setQuery(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Escape" && query) {
            event.stopPropagation();
            setQuery("");
          }
          // Down goes from the search into what it found, as it would in a native list.
          if (event.key === "ArrowDown") {
            event.preventDefault();
            document.querySelector<HTMLElement>(".settings [data-setting]")?.focus();
          }
        }}
      />
      {groups.length === 0 && <p className="settings-note">No shortcut matches</p>}
      {groups.map(({ group, actions }) => (
        <SettingsGroupBox key={group.id} title={group.title}>
          {actions.map((action) => (
            <SettingRow
              key={action.id}
              label={action.label}
              quiet={
                <MenuButton label={`Keys for ${action.label}`} className="icon-button" groups={menu(action)}>
                  <Icon name="more" />
                </MenuButton>
              }
            >
              {action.customized && <ResetButton label={action.label} onReset={() => reset(action.id)} />}
              <span className="key-chips">{keysOf(action)}</span>
            </SettingRow>
          ))}
        </SettingsGroupBox>
      ))}
    </SettingsPage>
  );
}
