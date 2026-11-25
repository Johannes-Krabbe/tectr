import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { ActivityItem, AgentMessage, LiveLine, type StepContext, SubagentsBack } from "./Activity";
import { useCommandSuggestions } from "./CommandList";
import { useFileSuggestions } from "./FileList";
import { ImageComposer, ImageViewer, type OpenImages, PromptBubble, useImageField } from "./ImageField";
import { highestNumber } from "./imageTokens";
import { motionAllowed } from "./appearance";
import {
  type AgentCommand,
  type Command,
  type AgentOption,
  type Approval,
  errorText,
  type PromptImage,
  type Provider,
  type Question,
  request,
  type Session,
  type Worktree,
  useSessionEvents,
} from "./engine";
import { Markdown } from "./markdown";
import { shellLine } from "./mentions";
import { act, type ActionId, isTextField, showsSteps, textBoxKey, titled } from "./keybinds";
import { inPane } from "./keys";
import {
  type AnswerEnd,
  branchBlocked,
  foldState,
  foldTranscript,
  liveStepLabel,
  NO_FOLDS,
  nextMode,
  planOf,
  sessionState,
  subagentLabel,
  subagentState,
  subagentsOf,
  suggestionOf,
  transcriptTiming,
  USAGE_SHOWN_FROM,
  usageNote,
  usageOf,
} from "./model";
import { FOLLOW_WITHIN, keepFollowing, newScroller } from "./motion";
import { PermissionCard } from "./PermissionCard";
import { QuestionCard } from "./QuestionCard";
import { SessionSettings } from "./SessionSettings";
import { refusedPrompt } from "./slashCommands";
import { useSyncedText } from "./syncedText";
import { Icon } from "./ui/Icon";
import { StatusIcon } from "./ui/StatusIcon";
import { useVim, VimModeLabel } from "./VimBox";
import { WorkPanel } from "./WorkPanel";

/** How far one press scrolls the transcript, in pixels. */
const SCROLL_STEP = 90;

export function AgentPane({
  machineId,
  session,
  provider,
  online,
  approvals,
  questions,
  options,
  draft,
  draftImages,
  commands,
  worktree,
  folder,
  onInterrupt,
  onBranched,
}: {
  machineId: string;
  session: Session;
  provider: Provider | undefined;
  online: boolean;
  approvals: Approval[];
  questions: Question[];
  /** The agent's own options, beyond the model, mode and effort. */
  options: AgentOption[];
  /** What's typed for this session but not sent yet, which any client can carry on with. */
  draft: string;
  /** The images the draft names. */
  draftImages: PromptImage[];
  /** The slash commands the agent last said it has. */
  commands: AgentCommand[] | undefined;
  worktree: Worktree | undefined;
  /** Where the agent works, which paths in its tools read from. */
  folder: string | undefined;
  /** Only there while the session has a turn to interrupt. */
  onInterrupt: (() => void) | undefined;
  /** A branch of this session opened as the tab with this ID. */
  onBranched: (sessionId: string) => void;
}) {
  const sessionRef = { machine_id: machineId, session_id: session.id };
  const events = useSessionEvents(sessionRef);
  // A subagent's transcript, read in place of the agent's own until going back.
  const [viewing, setViewing] = useState<string | null>(null);
  const items = useMemo(() => foldTranscript(events, viewing), [events, viewing]);
  const subagents = useMemo(() => subagentsOf(events), [events]);
  const plan = useMemo(() => planOf(events), [events]);
  const usage = usageOf(events);
  const suggestion = suggestionOf(events);
  const viewed = viewing === null ? undefined : subagents.get(viewing);
  const stepContext: StepContext = {
    subagents,
    folder,
    online,
    onOpenSubagent: (id) => {
      setViewing(id);
      following.current = true;
    },
    onStopSubagent: (id) => stopSubagent(id),
  };
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
  const questionBox = useRef<HTMLDivElement>(null);
  const vim = useVim(input);
  const [viewingImages, setViewingImages] = useState<{ images: PromptImage[]; start: number } | null>(
    null,
  );
  const openImages: OpenImages = (images, start) => {
    if (images.length > 0) setViewingImages({ images, start });
  };
  const [caret, setCaret] = useState(0);
  const followCaret = () => setCaret(input.current?.selectionStart ?? 0);
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
  const log = useRef<HTMLDivElement>(null);
  const content = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const lastTop = useRef(0);
  const [scroller] = useState(() => newScroller(() => log.current, motionAllowed));
  const pending = approvals[0];
  const question = pending ? undefined : questions[0];
  // Whether what's asked takes the keyboard when it arrives. Not from the message box, where
  // the Enter that was going to send would answer it instead. Read as it arrives, while the
  // box it replaces still has the keyboard.
  const askingId = pending?.id ?? question?.id ?? null;
  const asking = useRef({ id: askingId, take: true });
  if (asking.current.id !== askingId) {
    asking.current = { id: askingId, take: document.activeElement !== input.current };
  }

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

  const state = viewed ? subagentState(viewed.status, online) : sessionState(session, online);
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
  const files = useFileSuggestions({
    machineId,
    sessionId: session.id,
    text: message.text,
    caret,
    vimMode: vim.shown,
    onChange: (text, at) => {
      message.change(text);
      setCaret(at);
      requestAnimationFrame(() => input.current?.setSelectionRange(at, at));
    },
  });
  const shell = shellLine(message.text);
  function runCommand(command: Command) {
    setError(null);
    request(machineId, command).catch((reason) => setError(errorText(reason)));
  }
  function stopSubagent(id: string) {
    runCommand({ command: "stop_subagent", session_id: session.id, subagent: id });
  }

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
    // The transcript gets shorter when what's under it grows, like the message box taking
    // another line or a question taking its place. The text stays put and the bottom drops
    // out of sight, so a reader at the bottom is kept there, at once rather than eased.
    // Whether it was at the bottom is read from where it was before the change, because
    // following can be off while the reader is at the bottom anyway: a trackpad's bounce
    // there scrolls up without moving.
    let height = log.current?.clientHeight ?? 0;
    const box = new ResizeObserver(() => {
      const el = log.current;
      if (!el) return;
      const wasAtBottom = el.scrollHeight - el.scrollTop - height < FOLLOW_WITHIN;
      height = el.clientHeight;
      if (!following.current && !wasAtBottom) return;
      following.current = true;
      scroller.jump(el.scrollHeight - el.clientHeight);
      lastTop.current = el.scrollTop;
    });
    if (log.current) box.observe(log.current);
    return () => {
      observer.disconnect();
      box.disconnect();
      scroller.stop();
    };
  }, []);

  // The transcript has the keyboard, so single keys work. This never takes focus from
  // anything holding it: the composer mid-sentence, the sidebar, or a sheet.
  useEffect(() => {
    const active = document.activeElement;
    if (!active || active === document.body) log.current?.focus();
  }, [askingId]);

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
      // A question has keys of its own for moving and choosing.
      if (!inPane(event) || inQuestion(event)) return;
      // A question takes the message box's place, so it's what writing goes to.
      if (act(event, "pane.compose", () => (questionBox.current ?? input.current)?.focus())) return;
      // In a text box, Ctrl+D and the like edit the text. Scrolling is for the transcript.
      if (!isTextField(event.target)) commands.some(([id, run]) => act(event, id, run));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Going back leaves a subagent's transcript before it leaves the pane.
  useEffect(() => {
    if (viewing === null) return;
    const onKey = (event: KeyboardEvent) => {
      if (!inPane(event) || isTextField(event.target) || inQuestion(event)) return;
      if (act(event, "pane.back", () => setViewing(null))) event.stopImmediatePropagation();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [viewing]);

  // The agent's status moved on, so an error about the old one no longer applies.
  useEffect(() => setError(null), [session.status]);

  /** Moves the session on to the agent's next mode, as Shift+Tab does in Claude Code. */
  const cycleMode = () => {
    const mode = provider && nextMode(provider.modes, session.mode);
    if (!mode || !online) return;
    setError(null);
    request(machineId, {
      command: "set_session_settings",
      session_id: session.id,
      mode,
      model: session.model,
      effort: session.effort ?? null,
    }).catch((reason) => setError(errorText(reason)));
  };

  async function send(typed = message.text) {
    const text = typed.trim();
    if (!text || sending) return;
    const line = shellLine(text);
    if (line !== null) {
      if (!line) return;
      setSending(true);
      setError(null);
      message.cancel();
      try {
        await request(machineId, { command: "run_shell", session_id: session.id, line });
        message.clear();
      } catch (reason) {
        setError(errorText(reason));
        message.change(message.text);
      } finally {
        setSending(false);
      }
      return;
    }
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
      {viewing !== null && (
        <div className="subagent-bar">
          <button
            type="button"
            className="button small"
            title={titled("Back to the agent", "pane.back")}
            onClick={() => setViewing(null)}
          >
            <Icon name="back" />
            Back
          </button>
          <StatusIcon state={state} />
          <span className="subagent-title">{subagentLabel(viewed)}</span>
          {viewed?.description && viewed.title && (
            <span className="meta subagent-description">{viewed.description}</span>
          )}
          {viewed?.status === "working" && (
            <button
              type="button"
              className="button small"
              disabled={!online}
              onClick={() => stopSubagent(viewing)}
            >
              Stop
            </button>
          )}
        </div>
      )}
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
          {items.length === 0 && !live && (
            <p className="empty">
              Nothing here yet
            </p>
          )}
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
                    context={stepContext}
                  />
                );
              case "note":
                return (
                  <div
                    key={item.key}
                    className={`note ${item.failed ? "bad" : ""}`}
                  >
                    {item.text}
                  </div>
                );
              case "plan":
                return (
                  <details key={item.key} className="plan-document" open>
                    <summary className="activity-line">Plan</summary>
                    <div className="agent-message">
                      <Markdown text={item.text} />
                    </div>
                  </details>
                );
              case "compaction":
                return item.summary ? (
                  <details key={item.key} className="plan-document">
                    <summary className="activity-line">{compactionLabel(item.status)}</summary>
                    <div className="agent-message">
                      <Markdown text={item.summary} />
                    </div>
                  </details>
                ) : (
                  <div key={item.key} className={`note ${item.status === "failed" ? "bad" : ""}`}>
                    {compactionLabel(item.status)}
                    {item.error && `. ${item.error}`}
                  </div>
                );
              case "subagents_back":
                return <SubagentsBack key={item.key} back={item.back} context={stepContext} />;
              case "shell":
                return (
                  <div key={item.key} className="shell-run">
                    <pre className="tool-input code">$ {item.command}</pre>
                    {item.output && <pre className="shell-output code">{item.output}</pre>}
                    {item.exitCode !== undefined && item.exitCode !== 0 && (
                      <div className="note bad">Exited with {item.exitCode}</div>
                    )}
                  </div>
                );
              case "answer_end": {
                // A subagent's answers are its parent's to branch from, not its own.
                if (viewing !== null) {
                  return item.note ? (
                    <div key={item.key} className="note">
                      {item.note}
                    </div>
                  ) : null;
                }
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
              onToggle={() => setFolds(fold.toggleTurn())}
            />
          )}
        </div>
      </div>
      <div className="dock">
        <WorkPanel plan={plan} />
        {/* A request or a question takes the message box's place until it's answered. */}
        {pending ? (
          <PermissionCard
            key={pending.id}
            machineId={machineId}
            approval={pending}
            from={pending.subagent ? subagentLabel(subagents.get(pending.subagent)) : undefined}
            folder={folder}
            focusRef={questionBox}
            takeFocus={asking.current.take}
            onLeave={() => log.current?.focus()}
          />
        ) : question ? (
          <QuestionCard
            key={question.id}
            machineId={machineId}
            question={question}
            from={question.subagent ? subagentLabel(subagents.get(question.subagent)) : undefined}
            focusRef={questionBox}
            takeFocus={asking.current.take}
            onLeave={() => log.current?.focus()}
          />
        ) : (
          suggestions.list ?? files.list
        )}
        {!pending && !question && (
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
              placeholder={suggestion ?? "Message the agent"}
              title={
                suggestion && !message.text
                  ? titled("Use the suggestion", "composer.complete_command")
                  : undefined
              }
              aria-label="Message the agent"
              autoCorrect="off"
              autoCapitalize="off"
              onChange={(event) => {
                message.change(event.target.value);
                setCaret(event.target.selectionStart);
              }}
              {...vim.props}
              {...suggestions.boxProps(files.focusProps(vim.props))}
              onSelect={() => {
                vim.props.onSelect();
                followCaret();
              }}
              onPaste={(event) => {
                if (!field.onPaste(event)) vim.props.onPaste(event);
              }}
              onKeyDown={(event) => {
                if (field.onKeyDown(event, vim.shown)) return;
                if (vim.onKeyDown(event)) return;
                if (suggestions.onKeyDown(event)) return;
                if (files.onKeyDown(event)) return;
                if (act(event, "composer.cycle_mode", cycleMode)) return;
                // An empty box takes the agent's suggestion for what to send next.
                if (
                  suggestion &&
                  !message.text &&
                  act(event, "composer.complete_command", () => message.change(suggestion))
                ) {
                  return;
                }
                textBoxKey(
                  event,
                  [
                    "composer.send",
                    () => {
                      if (suggestions.sendChosen()) return;
                      if (files.sendChosen()) return;
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
            <SessionSettings
              machineId={machineId}
              session={session}
              provider={provider}
              options={options}
              online={online}
              usage={usage}
              input={input}
              onError={setError}
            />
            <div className="composer-context">
              {shell !== null && (
                <span
                  className="composer-mode"
                  title="Runs in the topic's folder, and the agent sees what it prints"
                >
                  Shell
                </span>
              )}
              <VimModeLabel mode={vim.shown} />
              {/* Only once it's nearly full, when a compaction is near. */}
              {usage && usage.percent >= USAGE_SHOWN_FROM && (
                <span className="composer-usage" title={usageNote(usage)}>
                  {usage.percent}%
                </span>
              )}
              {worktree && (
                <span className="composer-branch code" title={worktree.path}>
                  {worktree.branch}
                </span>
              )}
              {/* One button for what pressing does now: Send once something's typed, which
                  queues while the agent works, or else Stop while it works. */}
              {message.text.trim() ? (
                <button
                  type="submit"
                  className="send"
                  title={titled("Send", "composer.send")}
                  aria-label="Send"
                  disabled={!online || sending}
                  // Focus stays in the box, so typing carries on after a click.
                  onMouseDown={(event) => event.preventDefault()}
                >
                  <Icon name="send" />
                </button>
              ) : (
                onInterrupt && (
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
                )
              )}
            </div>
          </div>
        </form>
        )}
        {error && <p className="error">{error}</p>}
      </div>
      {viewingImages && (
        <ImageViewer
          machineId={machineId}
          images={viewingImages.images}
          start={viewingImages.start}
          onClose={() => setViewingImages(null)}
        />
      )}
    </div>
  );
}

function compactionLabel(status: string): string {
  switch (status) {
    case "in_progress":
      return "Compacting the conversation";
    case "failed":
      return "Compacting the conversation failed";
    case "cancelled":
      return "Compacting the conversation was cancelled";
    default:
      return "Compacted the conversation";
  }
}

/** Whether a key went to the question in the message box's place. */
function inQuestion(event: KeyboardEvent): boolean {
  return event.target instanceof Element && event.target.closest(".question") !== null;
}
