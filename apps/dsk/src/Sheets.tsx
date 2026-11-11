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
import { invoke } from "@tauri-apps/api/core";
import {
  type ArchiveRisk,
  type CommandOutput,
  errorText,
  finishPairing,
  type Folder,
  pairDevice,
  type PairingRequest,
  request,
  type Session,
  stopPairing,
  type Topic,
  type Workspace,
} from "./engine";
import { act, type ActionId, keysLabel, remapNative } from "./keybinds";
import { archiveLosses, archiveSummary, type MoveChoice, sessionLabel } from "./model";
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
import { countdown, spacedDigits } from "./pairing";
import { Icon } from "./ui/Icon";
import { QrCode } from "./ui/QrCode";
import { Keys } from "./ui/StatusIcon";
import { showToast } from "./toast";

export type SheetRequest =
  | { kind: "workspace"; machineId: string; deviceName: string; local: boolean }
  | { kind: "close"; machineId: string; session: Session }
  | { kind: "rename"; machineId: string; topic: Topic }
  | {
      kind: "archive";
      machineId: string;
      workspace: Workspace;
      topic: Topic;
      risk: ArchiveRisk;
      /** Where the keyboard goes back to, because the row it was on is going away. */
      fromSidebar: boolean;
    }
  | { kind: "move"; machineId: string; topic: Topic; choice: MoveChoice }
  | { kind: "pairing"; machineId: string; deviceName: string; code: string; expiresAtMs: number }
  | { kind: "add_device" };

const CANCEL_KEY: [ActionId, string][] = [["sheet.cancel", "Escape"]];

/** Creates something with a name and defaults for the rest, or changes one in place. */
export function Sheet({
  where,
  action,
  hint,
  canSubmit,
  run,
  onClose,
  onDone,
  onKeyDown,
  focusAction,
  confirmKey,
  returnsFocus,
  hints = [],
  list,
  input,
  children,
}: {
  where: string;
  action: string;
  hint?: ReactNode;
  canSubmit: boolean;
  /** `null` goes on to the sheet's next step, which stays open. */
  run: () => Promise<CommandOutput | null>;
  onClose: () => void;
  onDone: (output: CommandOutput) => void;
  onKeyDown?: (event: KeyboardEvent<HTMLFormElement>) => void;
  /** For a sheet with nothing to type in, whose Enter should do what the sheet is for. */
  focusAction?: boolean;
  /** For a sheet whose action can't be undone. Only this key does it, and Cancel is the primary
   * button with the focus, so an Enter out of habit cancels. */
  confirmKey?: ActionId;
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
  const cancelButton = useRef<HTMLButtonElement>(null);
  const submitKey = confirmKey ?? "sheet.submit";
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Cancelling goes back to where you were. Read while rendering, because the input's
  // autoFocus has already moved focus by the time Base UI looks. Creating something
  // doesn't go back: the app moves focus to the new thing.
  const [opener] = useState(() => document.activeElement as HTMLElement | null);
  const created = useRef(false);

  useEffect(() => setError(null), [input]);

  useHints("overlay", [
    ...(canSubmit && !busy ? [keyHint(action, submitKey)] : []),
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
      if (output === null) {
        setBusy(false);
        return;
      }
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
          initialFocus={confirmKey ? cancelButton : focusAction ? actionButton : undefined}
          onKeyDownCapture={(event) => remapNative(event, CANCEL_KEY)}
        >
          <form
            onSubmit={submit}
            onKeyDown={(event) => {
              const form = event.currentTarget;
              // Sending the browser's own submit, because it would ignore a remapped Enter.
              if (act(event, submitKey, () => form.requestSubmit())) return;
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
                <Dialog.Close ref={cancelButton} className={`button ${confirmKey ? "primary" : ""}`}>
                  Cancel
                  <Keys action="sheet.cancel" />
                </Dialog.Close>
                <button
                  ref={actionButton}
                  className={`button ${confirmKey ? "" : "primary"}`}
                  type="submit"
                  disabled={!canSubmit || busy}
                >
                  {action}
                  <Keys action={submitKey} />
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
  const delay = shownValue("general.remote_path_delay");
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
      hint={again ? `${again} again to confirm` : undefined}
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

/** Asks once when archiving stops agents, and twice when it throws away work in git: the second
 * time with exactly what goes. Without either, archiving doesn't ask at all. */
function ArchiveSheet({
  sheet,
  onClose,
  onDone,
}: {
  sheet: Extract<SheetRequest, { kind: "archive" }>;
  onClose: () => void;
  onDone: (output: CommandOutput) => void;
}) {
  const losses = archiveLosses(sheet.risk);
  const [sure, setSure] = useState(losses.length === 0);
  return (
    <Sheet
      where={sure && losses.length ? `Archiving ${sheet.topic.name} loses this` : `Archive ${sheet.topic.name}`}
      action={sure ? (losses.length ? "Archive and lose it" : "Archive topic") : "Confirm"}
      hint={sure && losses.length ? "It can't be brought back." : archiveSummary(sheet.risk)}
      canSubmit
      confirmKey="sheet.confirm"
      run={async () => {
        if (!sure) {
          setSure(true);
          return null;
        }
        return request(sheet.machineId, {
          command: "archive_topic",
          topic_id: sheet.topic.id,
          force: true,
        });
      }}
      onClose={onClose}
      onDone={onDone}
    >
      {sure && losses.length > 0 && (
        <ul className="sheet-losses">
          {losses.map((loss) => (
            <li key={loss}>{loss}</li>
          ))}
        </ul>
      )}
    </Sheet>
  );
}

/** Moving stops agents, so a running one needs confirming. */
function MoveSheet({
  sheet,
  onClose,
  onDone,
}: {
  sheet: Extract<SheetRequest, { kind: "move" }>;
  onClose: () => void;
  onDone: (output: CommandOutput) => void;
}) {
  return (
    <Sheet
      where={`An agent in ${sheet.topic.name} is still running`}
      action="Stop and move"
      canSubmit
      focusAction
      returnsFocus
      run={() =>
        request(sheet.machineId, {
          command: "move_topic",
          topic_id: sheet.topic.id,
          to: sheet.choice.target,
          force: true,
        })
      }
      onClose={onClose}
      onDone={onDone}
    />
  );
}

/** Shows the code that lets machines ask to pair, and a new one each time it runs out. Once one
 * asks, it shows digits that are typed here to let it in, so someone who only saw the code
 * stays out. Closing stops the code and turns away every machine still waiting. The daemon
 * still takes a code for a short grace after the clock here reaches zero. */
function PairingSheet({
  sheet,
  requests,
  onClose,
  onDone,
}: {
  sheet: Extract<SheetRequest, { kind: "pairing" }>;
  requests: PairingRequest[];
  onClose: () => void;
  onDone: (output: CommandOutput) => void;
}) {
  const [pairing, setPairing] = useState({ code: sheet.code, expiresAtMs: sheet.expiresAtMs });
  const [now, setNow] = useState(Date.now);
  const [failure, setFailure] = useState<string | null>(null);
  const [digits, setDigits] = useState("");
  const renewing = useRef(false);
  const closed = useRef(false);
  const left = pairing.expiresAtMs - now;
  const asked = requests.length > 0;
  const typed = digits.replace(/\s/g, "");

  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 250);
    return () => clearInterval(timer);
  }, []);

  // Machines already waiting keep waiting while the code renews.
  useEffect(() => {
    if (left > 0 || renewing.current || failure !== null) return;
    renewing.current = true;
    request(sheet.machineId, { command: "start_pairing" })
      .then((output) => {
        if (output.output !== "pairing") return;
        // Closed while asking: the new code would sit open with nothing showing it.
        if (closed.current) {
          request(sheet.machineId, { command: "cancel_pairing" }).catch(() => {});
          return;
        }
        setPairing({ code: output.code, expiresAtMs: output.expires_at_ms });
      })
      .catch((reason) => setFailure(errorText(reason)))
      .finally(() => {
        renewing.current = false;
      });
  }, [left, failure, sheet.machineId]);

  const waiting =
    requests.length > 1
      ? `${requests.length} machines are waiting, so someone else has the code`
      : `Type the digits ${requests[0]?.name ?? ""} shows`;

  return (
    <Sheet
      where={`Pair another machine with ${sheet.deviceName}`}
      action={asked ? "Pair" : "Copy"}
      hint={
        failure ??
        (asked ? (
          waiting
        ) : (
          <span className="pairing-clock">
            <Icon name="expires" />
            {countdown(left)}
          </span>
        ))
      }
      canSubmit={asked ? /^\d{6}$/.test(typed) : true}
      focusAction={!asked}
      returnsFocus
      input={digits}
      run={async () => {
        if (!asked) {
          await invoke("write_clipboard", { text: pairing.code });
          showToast("Copied the code");
          return null;
        }
        const output = await request(sheet.machineId, {
          command: "confirm_pairing",
          digits: typed,
        });
        if (output.output === "paired") showToast(`Paired ${output.client.name}`);
        return output;
      }}
      onClose={() => {
        closed.current = true;
        // After a pairing there's nothing left to stop.
        request(sheet.machineId, { command: "cancel_pairing" }).catch(() => {});
        onClose();
      }}
      onDone={onDone}
    >
      {asked ? (
        <input
          className="sheet-name code pairing-code"
          autoFocus
          value={digits}
          placeholder="000 000"
          aria-label="Digits the other machine shows"
          inputMode="numeric"
          autoComplete="off"
          spellCheck={false}
          onChange={(event) => setDigits(event.target.value)}
        />
      ) : (
        <>
          <QrCode text={pairing.code} label="Pairing code as a QR code" />
          <input
            className="sheet-name code pairing-code"
            readOnly
            value={pairing.code}
            aria-label="Pairing code"
            spellCheck={false}
          />
        </>
      )}
    </Sheet>
  );
}

/** A code looks like `tectr-<key>-<secret>`, which is all this checks before asking. */
const looksLikeCode = (text: string) => /^tectr-[A-Za-z0-9]+-[A-Za-z0-9]+$/.test(text.trim());

/** Pastes a code, then shows the digits to type on that device until they are. */
function AddDeviceSheet({
  onClose,
  onDone,
}: {
  onClose: () => void;
  onDone: (output: CommandOutput) => void;
}) {
  const [code, setCode] = useState("");
  const [asked, setAsked] = useState<{ digits: string; machine: string } | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const closed = useRef(false);

  return (
    <Sheet
      where="Add a device"
      action="Pair"
      hint={
        asked
          ? `Type these on ${asked.machine}`
          : (failure ?? "Pair another machine shows the code there")
      }
      canSubmit={!asked && looksLikeCode(code)}
      run={async () => {
        setFailure(null);
        const prompt = await pairDevice(code.trim());
        setAsked(prompt);
        finishPairing()
          .then(() => {
            if (closed.current) return;
            showToast(`Paired with ${prompt.machine}`);
            onDone({ output: "done" });
            onClose();
          })
          .catch((reason) => {
            if (closed.current) return;
            setFailure(errorText(reason));
            setAsked(null);
          });
        return null;
      }}
      onClose={() => {
        closed.current = true;
        if (asked) stopPairing().catch(() => {});
        onClose();
      }}
      onDone={onDone}
      input={code}
    >
      {asked ? (
        <input
          className="sheet-name code pairing-code"
          readOnly
          value={spacedDigits(asked.digits)}
          aria-label="Digits to type on the other machine"
          spellCheck={false}
        />
      ) : (
        <input
          className="sheet-name"
          autoFocus
          value={code}
          placeholder="tectr-..."
          aria-label="Pairing code"
          spellCheck={false}
          autoCorrect="off"
          autoCapitalize="off"
          onChange={(event) => setCode(event.target.value)}
        />
      )}
    </Sheet>
  );
}

export function Sheets({
  sheet,
  workspaces,
  pairingRequests,
  onClose,
  onDone,
}: {
  sheet: SheetRequest;
  /** What the sheet's device already has. */
  workspaces: Workspace[];
  /** The machines waiting on the sheet's device to type their digits. */
  pairingRequests: PairingRequest[];
  onClose: () => void;
  onDone: (output: CommandOutput) => void;
}) {
  switch (sheet.kind) {
    case "workspace":
      return (
        <WorkspaceSheet sheet={sheet} workspaces={workspaces} onClose={onClose} onDone={onDone} />
      );
    case "close":
      return <CloseSheet sheet={sheet} onClose={onClose} onDone={onDone} />;
    case "rename":
      return <RenameSheet sheet={sheet} onClose={onClose} onDone={onDone} />;
    case "archive":
      return <ArchiveSheet sheet={sheet} onClose={onClose} onDone={onDone} />;
    case "move":
      return <MoveSheet sheet={sheet} onClose={onClose} onDone={onDone} />;
    case "pairing":
      return (
        <PairingSheet
          sheet={sheet}
          requests={pairingRequests}
          onClose={onClose}
          onDone={onDone}
        />
      );
    case "add_device":
      return <AddDeviceSheet onClose={onClose} onDone={onDone} />;
  }
}
