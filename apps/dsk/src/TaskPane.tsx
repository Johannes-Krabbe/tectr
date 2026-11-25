// A background task's tab: work the agent left running, like a dev server, which is a
// terminal it opened rather than you. It shows what the task writes, read again every few
// seconds while it runs and kept at its end unless you scroll up. Closing the tab stops it.

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { type BackgroundTask, errorText, request } from "./engine";
import { act, type ActionId } from "./keybinds";
import { inPane } from "./keys";
import { taskName, taskState } from "./model";
import { FOLLOW_WITHIN } from "./motion";
import { StatusIcon } from "./ui/StatusIcon";

/** How often a running task's output is read again. */
const OUTPUT_EVERY_MS = 2000;
const SCROLL_STEP = 90;

const STATUS_TEXT: Record<BackgroundTask["status"], string> = {
  running: "Running",
  completed: "Finished",
  failed: "Failed",
  stopped: "Stopped",
  unknown: "",
};

export function TaskPane({
  machineId,
  sessionId,
  task,
  from,
  online,
}: {
  machineId: string;
  sessionId: string;
  task: BackgroundTask;
  /** The tab whose agent started it, by name. */
  from: string;
  online: boolean;
}) {
  const [output, setOutput] = useState<{ text: string; truncated: boolean } | string | null>(null);
  const running = task.status === "running";

  useEffect(() => {
    if (!task.output_file) return;
    let current = true;
    const read = () =>
      request(machineId, { command: "read_task_output", session_id: sessionId, task_id: task.id })
        .then((answer) => {
          if (current && answer.output === "task_output") setOutput(answer);
        })
        .catch((reason) => {
          if (current) setOutput(errorText(reason));
        });
    void read();
    const timer = running && online ? setInterval(read, OUTPUT_EVERY_MS) : undefined;
    return () => {
      current = false;
      clearInterval(timer);
    };
  }, [machineId, sessionId, task.id, task.output_file, running, online]);

  const body = useRef<HTMLPreElement>(null);
  const following = useRef(true);
  useLayoutEffect(() => {
    const el = body.current;
    if (el && following.current) el.scrollTop = el.scrollHeight;
  }, [output]);

  // The pane's scrolling keys, as in a transcript.
  useEffect(() => {
    const scrollTo = (top: (el: HTMLElement) => number) => () => {
      const el = body.current;
      if (!el) return;
      const next = Math.max(0, top(el));
      following.current = el.scrollHeight - next - el.clientHeight < FOLLOW_WITHIN;
      el.scrollTop = next;
    };
    const scrollBy = (amount: (el: HTMLElement) => number) =>
      scrollTo((el) => el.scrollTop + amount(el));
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
      commands.some(([id, run]) => act(event, id, run));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const status = STATUS_TEXT[task.status];
  const text =
    typeof output === "string"
      ? null
      : !task.output_file
        ? "This task doesn't say where its output goes"
        : output === null
          ? "Reading..."
          : `${output.truncated ? "...\n" : ""}${output.text || "Nothing yet"}`;
  return (
    <div className="pane task-pane">
      <div className="task-head">
        <StatusIcon state={taskState(task.status, online)} />
        <span className="task-name">{taskName(task)}</span>
        {task.command && task.description && (
          <span className="task-command code">{task.command}</span>
        )}
        <span className="meta task-meta">
          {[status, `from ${from}`].filter(Boolean).join(", ")}
        </span>
      </div>
      {task.summary && <p className="note task-summary">{task.summary}</p>}
      {typeof output === "string" && <p className="error main-error">{output}</p>}
      {text !== null && (
        <pre
          ref={body}
          className="task-body code"
          tabIndex={0}
          aria-label={`Output of ${taskName(task)}`}
          onScroll={(event) => {
            const el = event.currentTarget;
            following.current = el.scrollHeight - el.scrollTop - el.clientHeight < FOLLOW_WITHIN;
          }}
        >
          {text}
        </pre>
      )}
    </div>
  );
}
