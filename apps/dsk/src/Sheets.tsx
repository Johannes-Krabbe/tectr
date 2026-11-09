import { Dialog } from "@base-ui/react/dialog";
import {
  type FormEvent,
  type KeyboardEvent,
  type ReactNode,
  useEffect,
  useId,
  useRef,
  useState,
} from "react";
import { shownValue } from "./appearance";
import {
  type CommandOutput,
  errorText,
  type Folder,
  type PermissionMode,
  request,
  type Session,
  type Topic,
  type Workspace,
} from "./engine";
import { act, type ActionId, keysLabel, remapNative } from "./keybinds";
import { deleteTopicEffect, sessionLabel } from "./model";
import {
  afterPaste,
  highlightWhileTyping,
  moveHighlight,
  splitPath,
  STARTING_PATH,
  suggest,
  type Suggestion,
} from "./pathInput";
import { hint as keyHint, type Hint, useHints } from "./shortcuts";
import { Keys } from "./ui/StatusIcon";

export type SheetRequest =
  | { kind: "workspace"; machineId: string; deviceName: string; local: boolean }
  | { kind: "agent"; machineId: string; topic: Topic }
  | { kind: "close"; machineId: string; session: Session }
  | { kind: "rename"; machineId: string; topic: Topic }
  | {
      kind: "delete";
      machineId: string;
      workspace: Workspace;
      topic: Topic;
      /** Where the keyboard goes back to, because the row it was on is going away. */
      fromSidebar: boolean;
    };

export const MODES: { mode: PermissionMode; label: string; effect: string }[] = [
  { mode: "auto", label: "Auto", effect: "The agent decides what needs asking" },
  { mode: "manual", label: "Manual", effect: "Asks before making changes" },
  { mode: "yolo", label: "Yolo", effect: "Never asks before running anything" },
];

const CANCEL_KEY: [ActionId, string][] = [["sheet.cancel", "Escape"]];

const modeAction = (index: number) => `sheet.mode_${index + 1}` as ActionId;

/** Creates something with a name and defaults for the rest. */
function Sheet({
  where,
  action,
  hint,
  canSubmit,
  run,
  onClose,
  onDone,
  onKeyDown,
  focusAction,
  returnsFocus,
  hints = [],
  list,
  input,
  children,
}: {
  where: string;
  action: string;
  hint?: string;
  canSubmit: boolean;
  run: () => Promise<CommandOutput>;
  onClose: () => void;
  onDone: (output: CommandOutput) => void;
  onKeyDown?: (event: KeyboardEvent<HTMLFormElement>) => void;
  /** For a sheet with nothing to type in, whose Enter should do what the sheet is for. */
  focusAction?: boolean;
  /** For a sheet that changes something in place, so there's no new thing for focus to go to. */
  returnsFocus?: boolean;
  /** The keys only this sheet has, for the shortcut line. */
  hints?: Hint[];
  /** For a sheet that's an input and a list, like the palette. It takes the place of the
   * buttons, and an error takes its place. */
  list?: ReactNode;
  /** What's typed, when changing it should take back an error about the last try. */
  input?: string;
  children?: ReactNode;
}) {
  const actionButton = useRef<HTMLButtonElement>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Cancelling goes back to where you were. Read while rendering, because the input's
  // autoFocus has already moved focus by the time Base UI looks. Creating something
  // doesn't go back: the app moves focus to the new thing.
  const [opener] = useState(() => document.activeElement as HTMLElement | null);
  const created = useRef(false);

  useEffect(() => setError(null), [input]);

  useHints("overlay", [
    ...(canSubmit && !busy ? [keyHint(action, "sheet.submit")] : []),
    ...hints,
    keyHint("Cancel", "sheet.cancel"),
  ]);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!canSubmit || busy) return;
    setBusy(true);
    setError(null);
    try {
      const output = await run();
      created.current = !returnsFocus;
      onDone(output);
      onClose();
    } catch (reason) {
      setError(errorText(reason));
      setBusy(false);
    }
  };

  return (
    <Dialog.Root open onOpenChange={(open) => !open && onClose()}>
      <Dialog.Portal>
        <Dialog.Backdrop className="scrim" />
        <Dialog.Popup
          className="sheet"
          finalFocus={() => (created.current ? false : opener)}
          initialFocus={focusAction ? actionButton : undefined}
          onKeyDownCapture={(event) => remapNative(event, CANCEL_KEY)}
        >
          <form
            onSubmit={submit}
            onKeyDown={(event) => {
              const form = event.currentTarget;
              // Sending the browser's own submit, because it would ignore a remapped Enter.
              if (act(event, "sheet.submit", () => form.requestSubmit())) return;
              if (event.key === "Enter" && !(event.target instanceof HTMLButtonElement)) {
                event.preventDefault();
              }
              onKeyDown?.(event);
            }}
          >
            <Dialog.Title className="sheet-where">{where}</Dialog.Title>
            {children}
            {error ? <p className="error sheet-error">{error}</p> : list}
            {list === undefined && (
              <div className="sheet-foot">
                <span className="sheet-hint">{hint}</span>
                <Dialog.Close className="button">
                  Cancel
                  <Keys action="sheet.cancel" />
                </Dialog.Close>
                <button
                  ref={actionButton}
                  className="button primary"
                  type="submit"
                  disabled={!canSubmit || busy}
                >
                  {action}
                  <Keys action="sheet.submit" />
                </button>
              </div>
            )}
          </form>
        </Dialog.Popup>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

type Listing = { folders: Folder[] } | { error: string };

/** What's in a folder on a device, or undefined until that's known. Each folder is asked about
 * once, so typing within one folder never waits for the device, and only going into a new one
 * can. Answers are kept by folder, so a late one can't show up under the wrong path. */
function useListing(
  machineId: string,
  parent: string | undefined,
  delayMs: number,
): Listing | undefined {
  const [listings, setListings] = useState<Record<string, Listing>>({});
  const known = parent !== undefined && parent in listings;

  useEffect(() => {
    if (parent === undefined || known) return;
    const ask = async () => {
      let listing: Listing;
      try {
        const output = await request(machineId, { command: "list_folders", path: parent });
        listing = { folders: output.output === "folders" ? output.folders : [] };
      } catch (reason) {
        listing = { error: errorText(reason) };
      }
      setListings((current) => ({ ...current, [parent]: listing }));
    };
    if (delayMs === 0) {
      void ask();
      return;
    }
    const timer = setTimeout(() => void ask(), delayMs);
    return () => clearTimeout(timer);
  }, [machineId, parent, known, delayMs]);

  return parent === undefined ? undefined : listings[parent];
}

const WORKSPACE_HINTS = [
  keyHint("Move", "sheet.up", "sheet.down"),
  keyHint("Complete", "sheet.complete"),
];

const remoteDelay = () => {
  const delay = shownValue("layout.remote_path_delay");
  return typeof delay === "number" ? delay : 150;
};

function WorkspaceSheet({
  sheet,
  workspaces,
  onClose,
  onDone,
}: {
  sheet: Extract<SheetRequest, { kind: "workspace" }>;
  workspaces: Workspace[];
  onClose: () => void;
  onDone: (output: CommandOutput) => void;
}) {
  const [path, setPath] = useState(STARTING_PATH);
  const [highlight, setHighlight] = useState<number | null>(null);
  const caretPlaced = useRef(false);
  const list = useRef<HTMLDivElement>(null);
  const listId = useId();

  const split = splitPath(path);
  const listing = useListing(sheet.machineId, split?.parent, sheet.local ? 0 : remoteDelay());
  const rows =
    split && listing && "folders" in listing
      ? suggest(
          split.parent,
          split.partial,
          listing.folders,
          workspaces.map((workspace) => workspace.path),
        )
      : [];
  const chosen = highlight === null ? undefined : rows[highlight];

  useEffect(() => {
    if (highlight !== null) list.current?.children[highlight]?.scrollIntoView({ block: "nearest" });
  }, [highlight]);

  const edit = (next: string) => {
    setPath(next);
    setHighlight(highlightWhileTyping(splitPath(next)?.partial));
  };

  const complete = (row: Suggestion | undefined) => {
    if (row) edit(row.typed);
  };

  // There's nothing to go on to with a folder that's already a workspace, so Enter goes there.
  // Going further in is still what the complete key does.
  const completesOnEnter = chosen !== undefined && chosen.tag !== "added";

  return (
    <Sheet
      where={`New workspace on ${sheet.deviceName}`}
      action={completesOnEnter ? "Complete" : chosen ? "Open workspace" : "Add workspace"}
      canSubmit={path.trim() !== ""}
      run={() =>
        request(sheet.machineId, {
          command: "add_workspace",
          path: chosen ? chosen.path : path.trim(),
        })
      }
      onClose={onClose}
      onDone={onDone}
      hints={WORKSPACE_HINTS}
      input={path}
      list={
        listing && "error" in listing ? (
          <p className="error sheet-error">{listing.error}</p>
        ) : rows.length > 0 ? (
          <div ref={list} id={listId} className="palette-list" role="listbox">
            {rows.map((row, index) => (
              <div
                key={row.path}
                id={`${listId}-${index}`}
                className="palette-item"
                role="option"
                aria-selected={index === highlight}
                data-highlighted={index === highlight ? "" : undefined}
                onMouseDown={(event) => {
                  // The input keeps the keyboard, so you can keep typing after pointing at one.
                  event.preventDefault();
                  // A row can move under a still pointer, and a click means the row it lands on.
                  setHighlight(index);
                }}
                onMouseMove={() => setHighlight(index)}
                onClick={(event) => {
                  if (row.tag === "added") event.currentTarget.closest("form")?.requestSubmit();
                  else complete(row);
                }}
              >
                <span className="name code">{row.typed}</span>
                {row.tag && <span className="meta">{row.tag}</span>}
              </div>
            ))}
          </div>
        ) : null
      }
    >
      <input
        className="sheet-name code"
        autoFocus
        value={path}
        placeholder="/path/to/repo"
        aria-label="Repository path"
        role="combobox"
        aria-autocomplete="list"
        aria-expanded={rows.length > 0}
        aria-controls={listId}
        aria-activedescendant={chosen ? `${listId}-${highlight}` : undefined}
        spellCheck={false}
        autoCorrect="off"
        autoCapitalize="off"
        onChange={(event) => edit(event.target.value)}
        onFocus={(event) => {
          // Focusing a filled input can leave the caret at its start or select it all.
          if (caretPlaced.current) return;
          caretPlaced.current = true;
          const end = event.target.value.length;
          event.target.setSelectionRange(end, end);
        }}
        onPaste={(event) => {
          const replaced = afterPaste(path, event.clipboardData.getData("text"));
          if (replaced === null) return;
          event.preventDefault();
          edit(replaced);
        }}
        onKeyDown={(event) => {
          // Enter also confirms an IME composition.
          if (event.nativeEvent.isComposing) return;
          const move = (step: 1 | -1) => setHighlight(moveHighlight(highlight, step, rows.length));
          if (act(event, "sheet.up", () => move(-1))) return;
          if (act(event, "sheet.down", () => move(1))) return;
          if (act(event, "sheet.complete", () => complete(chosen ?? rows[0]))) return;
          // Anything else reaches the form, where this key submits.
          if (completesOnEnter) act(event, "sheet.submit", () => complete(chosen));
        }}
      />
    </Sheet>
  );
}

function AgentSheet({
  sheet,
  onClose,
  onDone,
}: {
  sheet: Extract<SheetRequest, { kind: "agent" }>;
  onClose: () => void;
  onDone: (output: CommandOutput) => void;
}) {
  const [agent, setAgent] = useState("claude");
  const [mode, setMode] = useState<PermissionMode>("auto");
  const [model, setModel] = useState("");
  const worktree = sheet.topic.worktrees[0];
  return (
    <Sheet
      where={`New agent in ${sheet.topic.name}`}
      action="Start agent"
      hint={MODES.find((option) => option.mode === mode)?.effect}
      canSubmit={agent.trim() !== "" && worktree !== undefined}
      run={() =>
        request(sheet.machineId, {
          command: "start_session",
          topic_id: sheet.topic.id,
          worktree_id: worktree.id,
          agent: agent.trim(),
          mode,
          model: model.trim() || null,
        })
      }
      onClose={onClose}
      onDone={onDone}
      hints={[keyHint("Permission mode", "sheet.mode_1", "sheet.mode_2", "sheet.mode_3")]}
      onKeyDown={(event) => {
        MODES.some((option, index) => act(event, modeAction(index), () => setMode(option.mode)));
      }}
    >
      <input
        className="sheet-name"
        autoFocus
        value={agent}
        aria-label="Agent"
        spellCheck={false}
        autoCorrect="off"
        autoCapitalize="off"
        onFocus={(event) => event.target.select()}
        onChange={(event) => setAgent(event.target.value)}
      />
      <input
        className="sheet-small"
        value={model}
        placeholder="Model, or leave empty for the agent's default"
        aria-label="Model"
        spellCheck={false}
        autoCorrect="off"
        autoCapitalize="off"
        onChange={(event) => setModel(event.target.value)}
      />
      <fieldset className="chips">
        <legend className="visually-hidden">Permission mode</legend>
        {MODES.map((option, index) => (
          <label key={option.mode} className="chip">
            <input
              type="radio"
              name="mode"
              className="visually-hidden"
              checked={mode === option.mode}
              onChange={() => setMode(option.mode)}
            />
            {option.label}
            <Keys action={modeAction(index)} />
          </label>
        ))}
      </fieldset>
    </Sheet>
  );
}

/** The second confirmation for closing a tab whose agent is still running. */
function CloseSheet({
  sheet,
  onClose,
  onDone,
}: {
  sheet: Extract<SheetRequest, { kind: "close" }>;
  onClose: () => void;
  onDone: (output: CommandOutput) => void;
}) {
  const again = keysLabel("global.close_tab");
  return (
    <Sheet
      where={`${sessionLabel(sheet.session)} is still running`}
      action="Stop and close"
      hint={
        again
          ? `Closing its tab stops the agent. Press ${again} again to confirm.`
          : "Closing its tab stops the agent."
      }
      canSubmit
      focusAction
      run={() =>
        request(sheet.machineId, {
          command: "close_session",
          session_id: sheet.session.id,
          force: true,
        })
      }
      onClose={onClose}
      onDone={onDone}
      hints={[keyHint("Confirm", "global.close_tab")]}
      onKeyDown={(event) => {
        const form = event.currentTarget;
        act(event, "global.close_tab", () => form.requestSubmit());
      }}
    />
  );
}

function RenameSheet({
  sheet,
  onClose,
  onDone,
}: {
  sheet: Extract<SheetRequest, { kind: "rename" }>;
  onClose: () => void;
  onDone: (output: CommandOutput) => void;
}) {
  const [name, setName] = useState(sheet.topic.name);
  const chosen = name.trim();
  return (
    <Sheet
      where={`Rename ${sheet.topic.name}`}
      action="Rename"
      canSubmit={chosen !== ""}
      returnsFocus
      // Confirming the name it has isn't choosing it, so it can go on following its first tab.
      run={async () =>
        chosen === sheet.topic.name
          ? { output: "done" }
          : request(sheet.machineId, {
              command: "rename_topic",
              topic_id: sheet.topic.id,
              name: chosen,
            })
      }
      onClose={onClose}
      onDone={onDone}
      input={name}
    >
      <input
        className="sheet-name"
        autoFocus
        value={name}
        aria-label="Topic name"
        spellCheck={false}
        onFocus={(event) => event.target.select()}
        onChange={(event) => setName(event.target.value)}
      />
    </Sheet>
  );
}

/** The sheet is the confirmation, so a running agent doesn't need asking about again. */
function DeleteSheet({
  sheet,
  onClose,
  onDone,
}: {
  sheet: Extract<SheetRequest, { kind: "delete" }>;
  onClose: () => void;
  onDone: (output: CommandOutput) => void;
}) {
  return (
    <Sheet
      where={`Delete ${sheet.topic.name}`}
      action="Delete topic"
      hint={deleteTopicEffect(sheet.topic, sheet.workspace.path)}
      canSubmit
      focusAction
      run={() =>
        request(sheet.machineId, {
          command: "delete_topic",
          topic_id: sheet.topic.id,
          force: true,
        })
      }
      onClose={onClose}
      onDone={onDone}
    />
  );
}

export function Sheets({
  sheet,
  workspaces,
  onClose,
  onDone,
}: {
  sheet: SheetRequest;
  /** What the sheet's device already has. */
  workspaces: Workspace[];
  onClose: () => void;
  onDone: (output: CommandOutput) => void;
}) {
  switch (sheet.kind) {
    case "workspace":
      return (
        <WorkspaceSheet sheet={sheet} workspaces={workspaces} onClose={onClose} onDone={onDone} />
      );
    case "agent":
      return <AgentSheet sheet={sheet} onClose={onClose} onDone={onDone} />;
    case "close":
      return <CloseSheet sheet={sheet} onClose={onClose} onDone={onDone} />;
    case "rename":
      return <RenameSheet sheet={sheet} onClose={onClose} onDone={onDone} />;
    case "delete":
      return <DeleteSheet sheet={sheet} onClose={onClose} onDone={onDone} />;
  }
}
