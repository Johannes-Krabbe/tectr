import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { ActivityItem, AgentMessage, LiveLine } from "./Activity";
import { useCommandSuggestions } from "./CommandList";
import { ImageComposer, ImageViewer, type OpenImages, PromptBubble, useImageField } from "./ImageField";
import { highestNumber } from "./imageTokens";
import { motionAllowed } from "./appearance";
import {
  type AgentCommand,
  type Approval,
  errorText,
  type PermissionOption,
  type PromptImage,
  type Provider,
  request,
  type Session,
  useSessionEvents,
  type Worktree,
} from "./engine";
import {
  act,
  type ActionId,
  isTextField,
  keysLabel,
  showsSteps,
  textBoxKey,
  titled,
  toggleSteps,
} from "./keybinds";
import { inPane } from "./keys";
import {
  type AnswerEnd,
  branchBlocked,
  foldState,
  foldTranscript,
  liveStepLabel,
  NO_FOLDS,
  sessionState,
  transcriptTiming,
} from "./model";
import { FOLLOW_WITHIN, keepFollowing, newScroller } from "./motion";
import { useScreenPaletteItems } from "./Palette";
import { sessionPickerHints, SessionPickers } from "./SessionPickers";
import { hint, type Hint, optionNumbersHint, useHints } from "./shortcuts";
import { refusedPrompt } from "./slashCommands";
import { useSyncedText } from "./syncedText";
import { Icon } from "./ui/Icon";
import { Keys, StatusIcon } from "./ui/StatusIcon";
import { useVim, VimModeLabel } from "./VimBox";

/** How far one press scrolls the transcript, in pixels. */
const SCROLL_STEP = 90;

const SCROLL_HINTS: Hint[] = [
  hint("Scroll", "pane.up", "pane.down"),
  hint("Half screen", "pane.half_up", "pane.half_down"),
  hint("Whole screen", "pane.page_up", "pane.page_down"),
  hint("Top, bottom", "pane.first", "pane.last"),
];

const COMPOSER_HINTS: Hint[] = [
  hint("Send", "composer.send"),
  hint("New line", "composer.newline"),
  hint("Leave", "composer.blur"),
];

function primaryOption(options: PermissionOption[]): PermissionOption | undefined {
  return options.find((option) => option.kind === "allow_once") ?? options[0];
}

/** The action that answers with this option by what it does. Options without one, such
 * as a second "allow once", answer by number. */
function decisionAction(
  option: PermissionOption,
  options: PermissionOption[],
): ActionId | undefined {
  const first = (...kinds: PermissionOption["kind"][]) =>
    kinds.map((kind) => options.find((o) => o.kind === kind)).find(Boolean);
  if (option === first("allow_once")) return "permission.allow_once";
  if (option === first("allow_always")) return "permission.allow_always";
  if (option === first("reject_once", "reject_always")) return "permission.reject";
  return undefined;
}

const numberAction = (index: number) =>
  index < 9 ? (`permission.option_${index + 1}` as ActionId) : undefined;

/** The action to show on an option's button: what it does, or else its number. */
const optionAction = (option: PermissionOption, index: number, options: PermissionOption[]) => {
  const decision = decisionAction(option, options);
  return decision && keysLabel(decision) ? decision : numberAction(index);
};

function requestHints({ options }: Approval): Hint[] {
  return [
    ...options.flatMap((option, index) => {
      const action = optionAction(option, index, options);
      return action ? [hint(option.name, action)] : [];
    }),
    optionNumbersHint(options.length),
  ];
}

/** Something an agent needs from you. Its keys are live. */
function RequestCard({ machineId, approval }: { machineId: string; approval: Approval }) {
  const [error, setError] = useState<string | null>(null);
  const primary = primaryOption(approval.options);

  const answer = (option: PermissionOption) => {
    setError(null);
    request(machineId, {
      command: "answer_approval",
      approval_id: approval.id,
      option_id: option.id,
    }).catch((reason) => setError(errorText(reason)));
  };

  // The keys answer, but a key that types never does while you're typing: a request
  // can appear mid-sentence. By default a held key doesn't either, because it would
  // carry on to whichever request comes next. Both are up to the settings file.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!inPane(event)) return;
      approval.options.some((option, index) => {
        const decision = decisionAction(option, approval.options);
        if (decision && act(event, decision, () => answer(option))) return true;
        const number = numberAction(index);
        return number !== undefined && act(event, number, () => answer(option));
      });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  return (
    <div className="request">
      <div className="request-top">
        <StatusIcon state="waiting" />
        <span>Permission request</span>
      </div>
      <pre className="request-what">{approval.tool_call.title ?? "Untitled tool call"}</pre>
      <div className="actions">
        {approval.options.map((option, index) => {
          const action = optionAction(option, index, approval.options);
          return (
            <button
              key={option.id}
              className={`button ${option === primary ? "primary" : ""}`}
              onClick={() => answer(option)}
            >
              {option.name}
              {action && <Keys action={action} />}
            </button>
          );
        })}
      </div>
      {error && <p className="error">{error}</p>}
    </div>
  );
}

export function AgentPane({
  machineId,
  session,
  provider,
  online,
  approvals,
  draft,
  draftImages,
  commands,
  worktree,
  onInterrupt,
  onBranched,
}: {
  machineId: string;
  session: Session;
  provider: Provider | undefined;
  online: boolean;
  approvals: Approval[];
  /** What's typed for this session but not sent yet, which any client can carry on with. */
  draft: string;
  /** The images the draft names. */
  draftImages: PromptImage[];
  /** The slash commands the agent last said it has. */
  commands: AgentCommand[] | undefined;
  worktree: Worktree | undefined;
  /** Only there while the session has a turn to interrupt. */
  onInterrupt: (() => void) | undefined;
  /** A branch of this session opened as the tab with this ID. */
  onBranched: (sessionId: string) => void;
}) {
  const sessionRef = { machine_id: machineId, session_id: session.id };
  const events = useSessionEvents(sessionRef);
  const items = useMemo(() => foldTranscript(events), [events]);
  // The draft is saved with the images its text names at the time, which the field below
  // knows, and it's made after this.
  const imagesFor = useRef<(text: string) => PromptImage[]>(() => []);
  const message = useSyncedText(draft, (text) => {
    const images = imagesFor.current(text);
    // The session may have just been closed, and then there's nothing left to keep this for.
    request(machineId, { command: "set_draft", session_id: session.id, text, images }).catch(
      () => {},
    );
  });
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const input = useRef<HTMLTextAreaElement>(null);
  const vim = useVim(input);
  const [viewing, setViewing] = useState<{ images: PromptImage[]; start: number } | null>(null);
  const openImages: OpenImages = (images, start) => {
    if (images.length > 0) setViewing({ images, start });
  };
  const sentImages = useMemo(
    () => items.flatMap((item) => (item.kind === "prompt" ? item.images : [])),
    [items],
  );
  const field = useImageField({
    machineId,
    input,
    text: message.text,
    known: draftImages,
    sent: highestNumber(sentImages),
    onError: setError,
    onOpen: openImages,
  });
  imagesFor.current = field.imagesFor;
  const openSent = () => openImages(sentImages, sentImages.length - 1);
  // The key handler is set up once, so it reaches the current images through a ref.
  const openSentNow = useRef(openSent);
  openSentNow.current = openSent;
  const log = useRef<HTMLDivElement>(null);
  const content = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const lastTop = useRef(0);
  const [scroller] = useState(() => newScroller(() => log.current, motionAllowed));
  const pending = approvals[0];

  // Only what happens while you're here animates, and never what was already there.
  const [openedMs] = useState(() => Date.now());
  const { streaming, isFresh } = transcriptTiming(events, openedMs);
  const streamingNow = useRef(streaming);
  streamingNow.current = streaming;

  const animate = motionAllowed();
  const stepsShown = showsSteps();
  // Changing the setting starts over, so that it always means what it says.
  const [folds, setFolds] = useState(NO_FOLDS);
  useEffect(() => setFolds(NO_FOLDS), [stepsShown]);
  const fold = foldState(items, folds, stepsShown);
  // The key handler is set up once, so it reaches this render's folds through a ref.
  const toggleTurn = useRef(() => {});
  toggleTurn.current = () => setFolds(fold.toggleTurn());

  const state = sessionState(session, online);
  const live = state === "working" || state === "waiting";
  const last = items[items.length - 1];
  const liveActivity = live && last?.kind === "activity" ? last : null;

  const suggestions = useCommandSuggestions({
    text: message.text,
    commands: commands ?? [],
    vimMode: vim.shown,
    onChange: message.change,
    onSend: (text) => void send(text),
  });

  const lastAnswer = items.filter((item): item is AnswerEnd => item.kind === "answer_end").pop();
  const blocked = (answer: AnswerEnd) => branchBlocked(answer, provider, session.agent, online);
  const branch = (answer: AnswerEnd) => {
    const reason = blocked(answer);
    if (reason) {
      setError(reason);
      return;
    }
    setError(null);
    request(machineId, { command: "fork_session", session_id: session.id, after_seq: answer.key })
      .then((output) => {
        if (output.output === "session") onBranched(output.session.id);
      })
      .catch((reason) => setError(errorText(reason)));
  };
  // The key handler is set up once, so it reaches the latest answer through a ref.
  const branchLast = useRef(() => {});
  branchLast.current = () => {
    if (lastAnswer) branch(lastAnswer);
  };

  const answers = pending ? requestHints(pending) : [];
  const pickers = sessionPickerHints(provider, session, online);
  useHints("pane", [
    ...answers,
    hint("Write", "pane.compose"),
    ...(sentImages.length > 0 ? [hint("Images", "pane.open_images")] : []),
    hint(stepsShown ? "Fold steps" : "Show steps", "pane.toggle_steps"),
    ...(lastAnswer ? [hint("Branch", "pane.branch")] : []),
    ...(live ? [hint(fold.turnOpen ? "Fold this turn" : "Watch", "pane.toggle_turn")] : []),
    ...SCROLL_HINTS,
    ...pickers,
    hint("Sidebar", "pane.back"),
  ]);
  // Keys that type never answer a request from here, and the line leaves those out itself.
  useHints("typing", [
    ...answers,
    ...suggestions.hints,
    ...COMPOSER_HINTS,
    ...(field.images.length > 0 ? [hint("Open image", "composer.open_image")] : []),
    ...pickers,
  ]);
  useScreenPaletteItems([
    ...(lastAnswer
      ? [
          {
            id: "branch",
            label: "Branch from the last answer into a new tab",
            detail: blocked(lastAnswer),
            keys: keysLabel("pane.branch"),
            run: () => branchLast.current(),
          },
        ]
      : []),
    ...(sentImages.length > 0
      ? [
          {
            id: "open-images",
            label: "Open the images in the conversation",
            keys: keysLabel("pane.open_images"),
            run: openSent,
          },
        ]
      : []),
    ...(field.images.length > 0
      ? [
          {
            id: "open-draft-images",
            label: "Open the images in the message box",
            keys: keysLabel("composer.open_image"),
            run: field.openAtCursor,
          },
        ]
      : []),
  ]);

  // Stay at the bottom unless the reader has scrolled up. This watches the size, because a
  // message keeps growing as it types out, long after the event that brought its text.
  useEffect(() => {
    if (!content.current) return;
    const observer = new ResizeObserver(() => {
      const el = log.current;
      if (!el) return;
      // Text getting shorter pulls the position up with it, and that isn't you moving.
      lastTop.current = el.scrollTop;
      if (!following.current) return;
      const bottom = el.scrollHeight - el.clientHeight;
      // Replaying history lands at its end, rather than scrolling through all of it.
      if (streamingNow.current) scroller.to(bottom);
      else scroller.jump(bottom);
    });
    observer.observe(content.current);
    return () => {
      observer.disconnect();
      scroller.stop();
    };
  }, []);

  // The transcript has the keyboard, so single keys work. This never takes focus from
  // anything holding it: the composer mid-sentence, the sidebar, or a sheet.
  useEffect(() => {
    const active = document.activeElement;
    if (!active || active === document.body) log.current?.focus();
  }, [pending]);

  // Moving between the transcript and the composer, and scrolling.
  useEffect(() => {
    const scrollTo = (top: (el: HTMLElement) => number) => () => {
      const el = log.current;
      if (!el) return;
      const next = top(el);
      // Decided here and not when it arrives, or new text would pull it back on the way.
      following.current = el.scrollHeight - next - el.clientHeight < FOLLOW_WITHIN;
      scroller.to(next);
    };
    const scrollBy = (amount: (el: HTMLElement) => number) =>
      scrollTo((el) => scroller.headedTo() + amount(el));
    const commands: [ActionId, () => void][] = [
      ["pane.up", scrollBy(() => -SCROLL_STEP)],
      ["pane.down", scrollBy(() => SCROLL_STEP)],
      ["pane.half_up", scrollBy((el) => -el.clientHeight / 2)],
      ["pane.half_down", scrollBy((el) => el.clientHeight / 2)],
      ["pane.page_up", scrollBy((el) => -el.clientHeight)],
      ["pane.page_down", scrollBy((el) => el.clientHeight)],
      ["pane.first", scrollTo(() => 0)],
      ["pane.last", scrollTo((el) => el.scrollHeight)],
    ];
    const onKey = (event: KeyboardEvent) => {
      if (!inPane(event)) return;
      if (act(event, "pane.compose", () => input.current?.focus())) return;
      const toggled = act(event, "pane.toggle_steps", () => {
        toggleSteps().catch((reason) => setError(errorText(reason)));
      });
      if (toggled) return;
      if (act(event, "pane.toggle_turn", () => toggleTurn.current())) return;
      if (act(event, "pane.open_images", () => openSentNow.current())) return;
      if (act(event, "pane.branch", () => branchLast.current())) return;
      // In a text box, Ctrl+D and the like edit the text. Scrolling is for the transcript.
      if (!isTextField(event.target)) commands.some(([id, run]) => act(event, id, run));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // The agent's status moved on, so an error about the old one no longer applies.
  useEffect(() => setError(null), [session.status]);

  async function send(typed = message.text) {
    const text = typed.trim();
    if (!text || sending) return;
    const refused = refusedPrompt(text);
    if (refused) {
      setError(refused);
      return;
    }
    setSending(true);
    setError(null);
    // The daemon clears the draft as it accepts this, and a push landing after that would
    // bring it back.
    message.cancel();
    try {
      const images = field.imagesFor(text);
      await request(machineId, { command: "send_prompt", session_id: session.id, text, images });
      message.clear();
      field.clear();
      // Sent, so back to reading what comes next.
      if (document.activeElement === input.current) log.current?.focus();
    } catch (reason) {
      setError(errorText(reason));
      message.change(message.text);
    } finally {
      setSending(false);
    }
  }

  return (
    <div className="pane">
      <div
        className="log"
        ref={log}
        tabIndex={0}
        onScroll={(event) => {
          const el = event.currentTarget;
          const movedUp = el.scrollTop < lastTop.current;
          lastTop.current = el.scrollTop;
          // On its way somewhere, it can be far from the bottom without you having left it.
          if (scroller.moving()) return;
          following.current = keepFollowing(
            following.current,
            el.scrollHeight - el.scrollTop - el.clientHeight,
            movedUp,
          );
        }}
        // Your own scrolling always wins over easing.
        onWheel={(event) => {
          scroller.stop();
          if (event.deltaY < 0) following.current = false;
        }}
        onPointerDown={() => scroller.stop()}
      >
        <div className="log-in" ref={content}>
          {items.length === 0 && !live && <p className="empty">Nothing here yet</p>}
          {items.map((item) => {
            switch (item.kind) {
              case "prompt":
                return (
                  <PromptBubble
                    key={item.key}
                    machineId={machineId}
                    text={item.text}
                    images={item.images}
                    onOpen={openImages}
                  />
                );
              case "message":
                return (
                  <AgentMessage
                    key={item.key}
                    text={item.text}
                    fresh={isFresh(item.key)}
                    animate={animate && streaming}
                  />
                );
              case "activity":
                return (
                  <ActivityItem
                    key={item.key}
                    activity={item}
                    live={item === liveActivity}
                    fresh={isFresh(item.key)}
                    open={fold.isOpen(item.key)}
                    onToggle={() => setFolds(fold.flip(item.key))}
                  />
                );
              case "note":
                return (
                  <div key={item.key} className={`note ${item.failed ? "bad" : ""}`}>
                    {item.text}
                  </div>
                );
              case "answer_end": {
                const reason = blocked(item);
                return (
                  <Fragment key={item.key}>
                    {item.note && <div className="note">{item.note}</div>}
                    <button
                      type="button"
                      className="branch"
                      disabled={reason !== undefined}
                      title={reason ?? "Branch from here into a new tab"}
                      onClick={() => branch(item)}
                    >
                      <Icon name="branch" />
                      Branch
                    </button>
                  </Fragment>
                );
              }
            }
          })}
          {live && (
            <LiveLine
              label={liveStepLabel(session, liveActivity)}
              waiting={state === "waiting"}
              animate={animate}
              open={fold.turnOpen}
              onToggle={() => toggleTurn.current()}
            />
          )}
        </div>
      </div>
      <div className="dock">
        {pending && <RequestCard machineId={machineId} approval={pending} />}
        {suggestions.list}
        <form
          className="composer"
          onSubmit={(event) => {
            event.preventDefault();
            void send();
          }}
        >
          <ImageComposer field={field} text={message.text} input={input} machineId={machineId}>
            <textarea
              ref={input}
              rows={1}
              value={message.text}
              placeholder={
                pending && keysLabel("composer.blur")
                  ? `Press ${keysLabel("composer.blur")} to answer`
                  : "Message the agent"
              }
              aria-label="Message the agent"
              autoCorrect="off"
              autoCapitalize="off"
              onChange={(event) => message.change(event.target.value)}
              {...vim.props}
              {...suggestions.boxProps(vim.props)}
              onPaste={(event) => {
                if (!field.onPaste(event)) vim.props.onPaste(event);
              }}
              onKeyDown={(event) => {
                if (field.onKeyDown(event, vim.shown)) return;
                if (vim.onKeyDown(event)) return;
                if (suggestions.onKeyDown(event)) return;
                textBoxKey(
                  event,
                  [
                    "composer.send",
                    () => {
                      if (suggestions.sendChosen()) return;
                      if (message.text.trim()) void send();
                      else log.current?.focus();
                    },
                  ],
                  "composer.newline",
                  ["composer.blur", () => log.current?.focus()],
                );
              }}
            />
          </ImageComposer>
          <div className="composer-foot">
            <SessionPickers
              machineId={machineId}
              session={session}
              provider={provider}
              online={online}
              input={input}
              transcript={log}
              onError={setError}
            />
            <div className="composer-context">
              <VimModeLabel mode={vim.shown} />
              {worktree && (
                <span className="composer-branch code" title={worktree.path}>
                  {worktree.branch}
                </span>
              )}
              {onInterrupt && (
                <button
                  type="button"
                  className="interrupt"
                  title={titled("Interrupt", "global.interrupt")}
                  aria-label="Interrupt"
                  // It goes away once the turn stops, which would leave focus nowhere.
                  onMouseDown={(event) => event.preventDefault()}
                  onClick={onInterrupt}
                >
                  <Icon name="stop" />
                </button>
              )}
            </div>
          </div>
        </form>
        {error && <p className="error">{error}</p>}
      </div>
      {viewing && (
        <ImageViewer
          machineId={machineId}
          images={viewing.images}
          start={viewing.start}
          onClose={() => setViewing(null)}
        />
      )}
    </div>
  );
}
