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
  type ClosedWorkspace,
  type Command,
  type CommandOutput,
  errorText,
  finishPairing,
  type Folder,
  pairDevice,
  type PairingRequest,
  type Repo,
  request,
  type Session,
  stopPairing,
  type Topic,
  type Workspace,
} from "./engine";
import { act, type ActionId, keysLabel, remapNative, titled } from "./keybinds";
import { archiveLosses, archiveSummary, type MoveChoice, sessionLabel } from "./model";
import {
  afterPaste,
  completesOnEnter,
  highlightWhileTyping,
  moveHighlight,
  splitPath,
  STARTING_PATH,
  suggest,
  suggestClones,
  suggestNew,
  type Suggestion,
} from "./pathInput";
import { countdown, spacedDigits } from "./pairing";
import { Icon } from "./ui/Icon";
import { QrCode } from "./ui/QrCode";
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
  | { kind: "add_device" }
  /** Something that can't be undone, which runs once confirmed. */
  | {
      kind: "confirm";
      machineId: string;
      where: string;
      /** What it does, when the action's name doesn't say enough. */
      hint?: string;
      action: string;
      command: Command;
      /** What happens once it's done, with the daemon's answer. */
      then?: (output: CommandOutput) => void;
    };

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
  dangerous,
  returnsFocus,
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
  /** For a sheet whose action can't be undone. Cancel is the primary button with the focus, so
   * an Enter out of habit cancels, and only Tab to the action and Enter, or a click, does it. */
  dangerous?: boolean;
  /** For a sheet that changes something in place, so there's no new thing for focus to go to. */
  returnsFocus?: boolean;
  /** For a sheet that's an input and a list. It takes the place of the
   * buttons, and an error takes its place. */
  list?: ReactNode;
  /** What's typed, when changing it should take back an error about the last try. */
  input?: string;
  children?: ReactNode;
}) {
  const actionButton = useRef<HTMLButtonElement>(null);
  const cancelButton = useRef<HTMLButtonElement>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Cancelling goes back to where you were. Read while rendering, because the input's
  // autoFocus has already moved focus by the time Base UI looks. Creating something
  // doesn't go back: the app moves focus to the new thing.
  const [opener] = useState(() => document.activeElement as HTMLElement | null);
  const created = useRef(false);

  useEffect(() => setError(null), [input]);

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
          initialFocus={dangerous ? cancelButton : focusAction ? actionButton : undefined}
          onKeyDownCapture={(event) => remapNative(event, CANCEL_KEY)}
        >
          <form
            onSubmit={submit}
            onKeyDown={(event) => {
              const form = event.currentTarget;
              // Sending the browser's own submit, because it would ignore a remapped Enter.
              if (!dangerous && act(event, "sheet.submit", () => form.requestSubmit())) return;
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
                <Dialog.Close
                  ref={cancelButton}
                  className={`button ${dangerous ? "primary" : ""}`}
                  title={titled("Cancel", "sheet.cancel")}
                >
                  Cancel
                </Dialog.Close>
                <button
                  ref={actionButton}
                  className={`button ${dangerous ? "" : "primary"}`}
                  type="submit"
                  title={dangerous ? undefined : titled(action, "sheet.submit")}
                  disabled={!canSubmit || busy}
                >
                  {action}
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

type RepoListing = { repos: Repo[]; problem?: string };

/** The device's GitHub repositories, asked for as the sheet opens, so they're usually there by
 * the time a name is typed. The daemon keeps them for a few minutes, so opening it again is
 * quick. */
function useRepos(machineId: string): RepoListing | undefined {
  const [listing, setListing] = useState<RepoListing>();
  useEffect(() => {
    let shown = true;
    request(machineId, { command: "list_repos" })
      .then((output) => {
        if (shown && output.output === "repos") {
          setListing({ repos: output.repos, problem: output.problem });
        }
      })
      .catch((reason) => {
        if (shown) setListing({ repos: [], problem: errorText(reason) });
      });
    return () => {
      shown = false;
    };
  }, [machineId]);
  return listing;
}

/** Clones without holding the sheet open, since a big repository takes minutes. The device's
 * page shows it going, and the daemon keeps a failure there until it's dismissed, so only what
 * went well is a toast. */
function startClone(
  machineId: string,
  url: string,
  path: string,
  onCloned: (workspace: Workspace) => void,
  onFailed: (message: string) => void,
) {
  request(machineId, { command: "clone_workspace", url, path }).then(
    (output) => {
      if (output.output !== "workspace") return;
      showToast(`${output.workspace.name} is cloned`);
      onCloned(output.workspace);
    },
    (reason) => onFailed(errorText(reason)),
  );
}

const remoteDelay = () => {
  const delay = shownValue("general.remote_path_delay");
  return typeof delay === "number" ? delay : 150;
};

/** Takes a path straight away. Under the folders it lists come the device's GitHub repositories
 * to clone there, then the typed name as a new folder, so opening, cloning and creating are one
 * box. MCP servers are chosen on the workspace's page, where adding one lands. */
function WorkspaceSheet({
  sheet,
  workspaces,
  onClose,
  onDone,
  onCloning,
  onCloned,
  onCloneFailed,
}: {
  sheet: Extract<SheetRequest, { kind: "workspace" }>;
  workspaces: Workspace[];
  onClose: () => void;
  onDone: (output: CommandOutput) => void;
  onCloning: (url: string) => void;
  onCloned: (workspace: Workspace) => void;
  onCloneFailed: (url: string, message: string) => void;
}) {
  // The device's closed workspaces, which adding one's folder opens again.
  const [closed, setClosed] = useState<ClosedWorkspace[]>([]);
  const [path, setPath] = useState(STARTING_PATH);
  const [highlight, setHighlight] = useState<number | null>(null);
  const caretPlaced = useRef(false);
  const list = useRef<HTMLDivElement>(null);
  const listId = useId();

  useEffect(() => {
    let shown = true;
    request(sheet.machineId, { command: "list_closed_workspaces" })
      .then((output) => {
        if (shown && output.output === "closed_workspaces") setClosed(output.workspaces);
      })
      // Without them it adds as it always does, and the daemon still opens a closed one.
      .catch(() => {});
    return () => {
      shown = false;
    };
  }, [sheet.machineId]);

  const repos = useRepos(sheet.machineId);
  const split = splitPath(path);
  const listing = useListing(
    sheet.machineId,
    split?.parent,
    sheet.local ? 0 : remoteDelay(),
  );
  const folders = split && listing && "folders" in listing ? listing.folders : undefined;
  const folderRows =
    split && folders
      ? suggest(
          split.parent,
          split.partial,
          folders,
          workspaces.map((workspace) => workspace.path),
          closed.map((workspace) => workspace.path),
        )
      : [];
  const cloneRows =
    split && folders && repos ? suggestClones(split.parent, split.partial, repos.repos, folders) : [];
  const newRows = split && folders ? suggestNew(split.parent, split.partial, folders) : [];
  const rows = [...folderRows, ...cloneRows, ...newRows];
  // Says why no repositories are offered, once a name is started and they would be.
  const noRepos = split?.partial ? repos?.problem : undefined;
  const chosen = highlight === null ? undefined : rows[highlight];
  const closedAt = (folder: string) =>
    closed.find((workspace) => workspace.path === folder.replace(/(.)\/+$/, "$1"));

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

  const entersFolder = completesOnEnter(chosen);
  const cloning = chosen?.tag === "clone" ? chosen.repo : undefined;
  const creating = chosen?.tag === "new";
  // A closed workspace comes back as it was, servers and all.
  const reopening =
    creating || cloning ? undefined : closedAt(chosen ? chosen.path : path.trim());

  return (
    <Sheet
      where={`New workspace on ${sheet.deviceName}`}
      action={
        entersFolder
          ? "Complete"
          : reopening
            ? "Open again"
            : cloning
              ? "Clone"
              : creating
                ? "Create workspace"
                : chosen
                  ? "Open workspace"
                  : "Add workspace"
      }
      hint={reopening && closedHint(reopening)}
      canSubmit={path.trim() !== ""}
      run={async () => {
        const chosenPath = chosen ? chosen.path : path.trim();
        if (cloning) {
          startClone(sheet.machineId, cloning.url, chosenPath, onCloned, (message) =>
            onCloneFailed(cloning.url, message),
          );
          onCloning(cloning.url);
          return { output: "done" };
        }
        return request(sheet.machineId, {
          command: creating ? "create_workspace" : "add_workspace",
          path: chosenPath,
        });
      }}
      onClose={onClose}
      onDone={onDone}
      input={path}
      list={
        listing && "error" in listing ? (
          <p className="error sheet-error">{listing.error}</p>
        ) : rows.length > 0 || noRepos ? (
          <>
            <div ref={list} id={listId} className="list-choices" role="listbox">
              {rows.map((row, index) => (
                <div
                  key={row.repo?.url ?? row.path}
                  id={`${listId}-${index}`}
                  className="list-choice"
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
                    if (completesOnEnter(row)) complete(row);
                    else event.currentTarget.closest("form")?.requestSubmit();
                  }}
                >
                  <span className="name code">{row.typed}</span>
                  {row.tag && (
                    <span className="meta">
                      {row.tag === "closed"
                        ? closedTag(closedAt(row.path))
                        : row.tag === "clone"
                          ? `clone ${row.repo?.full_name ?? ""}`
                          : row.tag === "new"
                            ? "new folder, git init"
                            : row.tag}
                    </span>
                  )}
                </div>
              ))}
            </div>
            {noRepos && <p className="sheet-note">No GitHub repositories to clone: {noRepos}</p>}
          </>
        ) : null
      }
    >
      <input
        className="sheet-name code"
        autoFocus
        value={path}
        placeholder="/path/to/folder"
        aria-label="Folder path"
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
          if (entersFolder) act(event, "sheet.submit", () => complete(chosen));
        }}
      />
    </Sheet>
  );
}

const closedOn = (ms: number) =>
  new Date(ms).toLocaleDateString(undefined, { month: "short", day: "numeric" });

/** Says what adding a closed workspace's folder brings back. */
const closedHint = (workspace: ClosedWorkspace) => {
  const topics = workspace.topics === 1 ? "1 topic" : `${workspace.topics} topics`;
  return `Closed ${closedOn(workspace.closed_at_ms)}, comes back with ${topics}`;
};

const closedTag = (workspace: ClosedWorkspace | undefined) =>
  workspace ? `closed, ${workspace.topics === 1 ? "1 topic" : `${workspace.topics} topics`}` : "closed";

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
      onKeyDown={(event) => {
        const form = event.currentTarget;
        act(event, "global.close_tab", () => form.requestSubmit());
      }}
    />
  );
}

function ConfirmSheet({
  sheet,
  onClose,
  onDone,
}: {
  sheet: Extract<SheetRequest, { kind: "confirm" }>;
  onClose: () => void;
  onDone: (output: CommandOutput) => void;
}) {
  return (
    <Sheet
      where={sheet.where}
      action={sheet.action}
      hint={sheet.hint}
      canSubmit
      dangerous
      returnsFocus
      run={() => request(sheet.machineId, sheet.command)}
      onClose={onClose}
      onDone={onDone}
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
      dangerous
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
  onCloning,
  onCloned,
  onCloneFailed,
}: {
  sheet: SheetRequest;
  /** What the sheet's device already has. */
  workspaces: Workspace[];
  /** The machines waiting on the sheet's device to type their digits. */
  pairingRequests: PairingRequest[];
  onClose: () => void;
  onDone: (output: CommandOutput) => void;
  /** A clone started from the workspace sheet, which closes straight away. */
  onCloning: (machineId: string, url: string) => void;
  /** That clone is a workspace now, minutes later perhaps. */
  onCloned: (machineId: string, workspace: Workspace) => void;
  /** That clone failed. Most failures also stay in the device's state, but not one refused
   * before it started, like a folder that's there already. */
  onCloneFailed: (machineId: string, url: string, message: string) => void;
}) {
  switch (sheet.kind) {
    case "workspace":
      return (
        <WorkspaceSheet
          sheet={sheet}
          workspaces={workspaces}
          onClose={onClose}
          onDone={onDone}
          onCloning={(url) => onCloning(sheet.machineId, url)}
          onCloned={(workspace) => onCloned(sheet.machineId, workspace)}
          onCloneFailed={(url, message) => onCloneFailed(sheet.machineId, url, message)}
        />
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
    case "confirm":
      return <ConfirmSheet sheet={sheet} onClose={onClose} onDone={onDone} />;
  }
}
