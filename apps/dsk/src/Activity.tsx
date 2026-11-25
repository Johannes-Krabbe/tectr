// What an agent does between its messages: one line while it works, which folds to how long
// it took. The messages around it are never folded.

import { type MouseEvent, type ReactNode, useEffect, useRef, useState } from "react";
import { Markdown } from "./markdown";
import {
  activitySummary,
  type Activity,
  type ActivityStep,
  formatWorked,
  type SubagentInfo,
  subagentLabel,
  subagentRunning,
  subagentState,
} from "./model";
import { nextTyped, typedText } from "./motion";
import { ToolDetails } from "./ToolDetails";
import { hasDetails } from "./toolView";
import { Icon } from "./ui/Icon";
import { StatusIcon } from "./ui/StatusIcon";
import { useNow } from "./ui/useNow";

/** The least time a step stays up, so that a run of quick tools can be read. */
const STEP_DWELL_MS = 320;
/** How long a step takes to fade out, which is `motion.fast` in the stylesheet. */
const STEP_OUT_MS = 80;

/** A label that fades out before the next one fades in, so the two never overlap. */
function useSteppedLabel(label: string) {
  const [step, setStep] = useState({ label, leaving: false });
  const shownAt = useRef(performance.now());
  const latest = useRef(label);
  latest.current = label;
  const changed = label !== step.label;

  useEffect(() => {
    if (!changed) return;
    if (!step.leaving) {
      const rest = shownAt.current + STEP_DWELL_MS - performance.now();
      const timer = setTimeout(() => setStep((s) => ({ ...s, leaving: true })), rest);
      return () => clearTimeout(timer);
    }
    const timer = setTimeout(() => {
      shownAt.current = performance.now();
      setStep({ label: latest.current, leaving: false });
    }, STEP_OUT_MS);
    return () => clearTimeout(timer);
  }, [changed, step]);

  return { label: step.label, leaving: changed && step.leaving };
}

/** The line at the end of a running transcript, saying what the agent is on. */
export function LiveLine({
  label,
  waiting,
  animate,
  open,
  onToggle,
}: {
  label: string;
  waiting: boolean;
  animate: boolean;
  /** Whether the turn it belongs to shows its steps, including ones still to come. */
  open: boolean;
  onToggle: () => void;
}) {
  const step = useSteppedLabel(label);
  const text = (
    <span
      key={step.label}
      className={[
        "activity-text",
        "fades-in",
        step.leaving && "leaving",
        animate && !waiting && "sweeps",
      ]
        .filter(Boolean)
        .join(" ")}
    >
      {step.label}
    </span>
  );
  return (
    <FoldButton open={open} onToggle={onToggle}>
      {text}
    </FoldButton>
  );
}

function FoldButton({
  open,
  onToggle,
  children,
}: {
  open: boolean;
  onToggle: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      className="activity-line"
      aria-expanded={open}
      // Unfolding leaves the keyboard wherever it was, such as in the composer.
      onMouseDown={(event: MouseEvent<HTMLButtonElement>) => event.preventDefault()}
      onClick={onToggle}
    >
      {children}
    </button>
  );
}

const TOOL_META: Record<string, string> = {
  pending: "Pending",
  in_progress: "Running",
  failed: "Failed",
};

/** What a step needs from around it: the subagents it can name, and opening and stopping
 * one. */
export interface StepContext {
  subagents: Map<string, SubagentInfo>;
  /** The session's folder, which paths read from. */
  folder: string | undefined;
  online: boolean;
  onOpenSubagent: (id: string) => void;
  onStopSubagent: (id: string) => void;
}

/** A subagent where it was started, which stays in sight while it runs, folded or not. It
 * counts how long it's been going, and its Stop shows on the row in use. */
function SubagentStep({ id, context }: { id: string; context: StepContext }) {
  const subagent = context.subagents.get(id);
  const running = subagentRunning(subagent);
  const now = useNow(running && context.online, 1000);
  return (
    <div className="subagent-step">
      <button
        type="button"
        className="tool tool-button"
        title="Show what the subagent did"
        onClick={() => context.onOpenSubagent(id)}
      >
        <StatusIcon state={subagentState(subagent?.status, context.online)} />
        <span className="tool-title">{subagentLabel(subagent)}</span>
        {subagent?.description && subagent.title && (
          <span className="meta tool-description">{subagent.description}</span>
        )}
      </button>
      {running && subagent && context.online && (
        <span className="meta step-time">{formatWorked(now - subagent.startedMs)}</span>
      )}
      {subagent?.status === "working" && (
        <span className="step-quiet">
          <button
            type="button"
            className="icon-button"
            title="Stop the subagent"
            aria-label="Stop the subagent"
            disabled={!context.online}
            onClick={() => context.onStopSubagent(id)}
          >
            <Icon name="remove" />
          </button>
        </span>
      )}
    </div>
  );
}

function Step({ step, live, context }: { step: ActivityStep; live: boolean; context: StepContext }) {
  const [open, setOpen] = useState(false);
  switch (step.kind) {
    case "thought":
      return <div className="thought">{step.text}</div>;
    case "note":
      return <div className="note">{step.text}</div>;
    case "subagent":
      return <SubagentStep id={step.id} context={context} />;
    case "tool": {
      const { status, title } = step.tool;
      // An interrupted turn can leave a tool saying it's running, and it no longer is.
      const meta = status && (live || status === "failed") ? TOOL_META[status] : undefined;
      const row = (
        <>
          <span className="tool-title">{title ?? "Tool call"}</span>
          {meta && <span className={status === "failed" ? "meta bad" : "meta"}>{meta}</span>}
        </>
      );
      if (!hasDetails(step.tool)) return <div className="tool">{row}</div>;
      return (
        <div className="tool-step">
          <button
            type="button"
            className="tool tool-button"
            aria-expanded={open}
            onClick={() => setOpen(!open)}
          >
            {row}
          </button>
          {open && <ToolDetails tool={step.tool} folder={context.folder} />}
        </div>
      );
    }
  }
}

/** Subagents that came back between the agent's messages, a line each, where they came back.
 * Pressing one shows what it did. */
export function SubagentsBack({
  back,
  context,
}: {
  back: { id: string; tookMs: number }[];
  context: StepContext;
}) {
  return (
    <div className="subagents-back">
      {back.map(({ id, tookMs }) => (
        <button
          key={id}
          type="button"
          className="subagent-back"
          title="Show what the subagent did"
          onClick={() => context.onOpenSubagent(id)}
        >
          <StatusIcon state="read" />
          <span className="subagent-back-name">{subagentLabel(context.subagents.get(id))}</span>
          <span className="subagent-back-time">finished after {formatWorked(tookMs)}</span>
        </button>
      ))}
    </div>
  );
}

/** An activity's steps, and when it's over, the line it folded into. While it's running
 * that line is `LiveLine`, at the end of the transcript. */
export function ActivityItem({
  activity,
  live,
  fresh,
  open,
  onToggle,
  context,
}: {
  activity: Activity;
  live: boolean;
  /** It finished while you were here, rather than being there when you came. */
  fresh: boolean;
  open: boolean;
  onToggle: () => void;
  context: StepContext;
}) {
  const summary = live ? null : activitySummary(activity);
  // Folded, it still shows the subagents that are going, including ones left running after
  // the turn ended.
  const steps = open
    ? activity.steps
    : activity.steps.filter(
        (step) => step.kind === "subagent" && subagentRunning(context.subagents.get(step.id)),
      );
  return (
    <div className="activity">
      {summary && (
        <FoldButton open={open} onToggle={onToggle}>
          <span className={fresh ? "activity-text fades-in" : "activity-text"}>
            {summary.text}
            {summary.failed > 0 && <span className="bad">, {summary.failed} failed</span>}
          </span>
        </FoldButton>
      )}
      {steps.length > 0 && (
        <div className="activity-steps">
          {steps.map((step) => (
            <Step key={step.key} step={step} live={live} context={context} />
          ))}
        </div>
      )}
    </div>
  );
}

/** How much of a message to show, moving up to all of it a letter at a time. */
function useTypedLength(length: number, fresh: boolean, animate: boolean): number {
  const [shown, setShown] = useState(fresh && animate ? 0 : length);
  const exact = useRef(shown);
  const latest = useRef(length);
  latest.current = length;
  // Keeps up while it isn't typing, so that typing later starts from what's on screen.
  if (!animate) {
    exact.current = length;
    if (shown !== length) setShown(length);
  }
  const behind = animate && shown < length;

  useEffect(() => {
    if (!behind) return;
    let last = performance.now();
    let frame = 0;
    const tick = (now: number) => {
      exact.current = nextTyped(exact.current, latest.current, now - last);
      last = now;
      setShown(Math.floor(exact.current));
      // Even when it has caught up: more may have come before this update lands, and if
      // it hasn't, `behind` goes false and this stops.
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [behind]);

  return animate ? shown : length;
}

/** A message, typing out as it streams. Whatever was there when it first showed stays as
 * it is. */
export function AgentMessage({
  text,
  fresh,
  animate,
}: {
  text: string;
  fresh: boolean;
  animate: boolean;
}) {
  const shown = useTypedLength(text.length, fresh, animate);
  return (
    <div className="agent-message">
      <Markdown text={typedText(text, shown)} />
    </div>
  );
}
