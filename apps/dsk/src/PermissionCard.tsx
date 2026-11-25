// A permission request in the message box's place, looking and moving like a question: what
// the agent wants to run, then its options as one list. The question keys move and choose, and
// each option's number answers with it straight away.

import { type KeyboardEvent as ReactKeyboardEvent, type RefObject, useEffect, useState } from "react";
import { type Approval, errorText, type PermissionOption, request } from "./engine";
import { act, type ActionId, keysLabel } from "./keybinds";
import { inPane } from "./keys";
import { ToolDetails } from "./ToolDetails";
import { hasDetails, type RequestHead, requestHead } from "./toolView";

// Answering is by number, which works for every request and question, whatever its options are.
export const OPTION_ACTIONS: ActionId[] = [
  "permission.option_1",
  "permission.option_2",
  "permission.option_3",
  "permission.option_4",
  "permission.option_5",
  "permission.option_6",
  "permission.option_7",
  "permission.option_8",
  "permission.option_9",
];

function primaryOption(options: PermissionOption[]): number {
  return Math.max(
    0,
    options.findIndex((option) => option.kind === "allow_once"),
  );
}

export function PermissionCard({
  machineId,
  approval,
  from,
  folder,
  focusRef,
  takeFocus,
  onLeave,
}: {
  machineId: string;
  approval: Approval;
  /** The session's folder, which paths read from. */
  folder: string | undefined;
  /** The subagent asking, by name. */
  from?: string;
  /** What entering the request focuses, which the pane's compose key does. */
  focusRef: RefObject<HTMLDivElement | null>;
  /** Whether it takes the keyboard on arriving. Not from someone typing, whose next Enter
   * would answer it. */
  takeFocus: boolean;
  /** Going back out of it, to the transcript. */
  onLeave: () => void;
}) {
  const [error, setError] = useState<string | null>(null);
  const [answering, setAnswering] = useState(false);
  const [highlight, setHighlight] = useState(() => primaryOption(approval.options));
  const options = approval.options;
  const at = Math.min(highlight, Math.max(0, options.length - 1));

  const answer = (option: PermissionOption | undefined) => {
    if (!option || answering) return;
    setError(null);
    setAnswering(true);
    request(machineId, {
      command: "answer_approval",
      approval_id: approval.id,
      option_id: option.id,
    }).catch((reason) => {
      setError(errorText(reason));
      setAnswering(false);
    });
  };

  // The message box it replaced had the keyboard, and that went with it.
  useEffect(() => {
    if (takeFocus && document.activeElement === document.body) focusRef.current?.focus();
  }, [approval.id, focusRef, takeFocus]);

  // Its numbers answer from anywhere in the pane, but a key that types never does while you're
  // typing, and by default a held key doesn't either, because it would carry on to whichever
  // request comes next. Both are up to the settings file.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!inPane(event)) return;
      options.some((option, index) => {
        const number = OPTION_ACTIONS[index];
        return number !== undefined && act(event, number, () => answer(option));
      });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const move = (step: 1 | -1) => setHighlight(Math.max(0, Math.min(options.length - 1, at + step)));
  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (act(event, "question.up", () => move(-1))) return;
    if (act(event, "question.down", () => move(1))) return;
    if (act(event, "question.choose", () => answer(options[at]))) return;
    act(event, "question.leave", onLeave);
  };

  const tool = approval.tool_call;
  const title = tool.title ?? "Untitled tool call";
  const head = requestHead(tool, folder);
  return (
    <div
      className="question"
      ref={focusRef}
      tabIndex={0}
      role="group"
      aria-label={from ? `${from}: ${title}` : title}
      onKeyDown={onKeyDown}
    >
      {from && (
        <div className="question-top">
          <span className="question-heading">{from}</span>
        </div>
      )}
      {head && <Head head={head} />}
      {hasDetails(tool) && (
        <div className="question-details">
          <ToolDetails tool={tool} folder={folder} />
        </div>
      )}
      <div className="question-options" role="group">
        {options.map((option, index) => {
          // Its number reads as where it is in the list, and is also the key that picks it.
          const action = OPTION_ACTIONS[index];
          return (
            <button
              key={option.id}
              type="button"
              // The list is one stop, reached through the request itself.
              tabIndex={-1}
              className="question-option"
              data-highlighted={index === at ? "" : undefined}
              disabled={answering}
              onMouseMove={() => setHighlight(index)}
              onClick={() => answer(option)}
            >
              <span className="question-number">{action && keysLabel(action)}</span>
              <span className="question-label">{option.name}</span>
            </button>
          );
        })}
      </div>
      {error && <p className="error">{error}</p>}
    </div>
  );
}

/** The one line saying what it would do: the file an edit changes and by how much, read like
 * a file in the diff tab, or what a command is for. */
function Head({ head }: { head: RequestHead }) {
  if (head.kind === "text") return <div className="question-text">{head.text}</div>;
  return (
    <div className="request-head">
      <span className="request-verb">{head.verb}</span>
      <span className="request-file code">{head.name}</span>
      {head.folder && <span className="request-folder code">{head.folder}</span>}
      <span className="request-counts code">
        {head.added > 0 && <span className="added">+{head.added}</span>}
        {head.removed > 0 && <span className="removed">-{head.removed}</span>}
      </span>
    </div>
  );
}
