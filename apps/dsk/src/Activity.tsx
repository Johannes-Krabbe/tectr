// What an agent does between its messages: one line while it works, which folds to how long
// it took. The messages around it are never folded.

import { type MouseEvent, type ReactNode, useEffect, useRef, useState } from "react";
import { Markdown } from "./markdown";
import { activitySummary, type Activity, type ActivityStep } from "./model";
import { nextTyped, typedText } from "./motion";

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
        waiting && "waiting",
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

function Step({ step, live }: { step: ActivityStep; live: boolean }) {
  switch (step.kind) {
    case "thought":
      return <div className="thought">{step.text}</div>;
    case "note":
      return <div className="note">{step.text}</div>;
    case "tool": {
      const { status, title } = step.tool;
      // An interrupted turn can leave a tool saying it's running, and it no longer is.
      const meta = status && (live || status === "failed") ? TOOL_META[status] : undefined;
      return (
        <div className="tool">
          <span className="tool-title">{title ?? "Tool call"}</span>
          {meta && <span className={status === "failed" ? "meta bad" : "meta"}>{meta}</span>}
        </div>
      );
    }
  }
}

/** An activity's steps, and when it's over, the line it folded into. While it's running
 * that line is `LiveLine`, at the end of the transcript. */
export function ActivityItem({
  activity,
  live,
  fresh,
  open,
  onToggle,
}: {
  activity: Activity;
  live: boolean;
  /** It finished while you were here, rather than being there when you came. */
  fresh: boolean;
  open: boolean;
  onToggle: () => void;
}) {
  const summary = live ? null : activitySummary(activity);
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
      {open && activity.steps.length > 0 && (
        <div className="activity-steps">
          {activity.steps.map((step) => (
            <Step key={step.key} step={step} live={live} />
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
