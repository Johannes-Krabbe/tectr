// A form an agent asked to have filled in, such as Claude's multiple-choice questions. It takes
// the message box's place and asks one question at a time, then ends, as Claude Code does, on a
// page for anything else to say, with Send under it.
//
// Each page is one list, top to bottom: the options, then the box for an answer of your own,
// then Next where choosing doesn't go on by itself, or on the last page the note and Send. The
// question keys move through all of it. Moving onto a box only highlights it, so moving can go
// on past it, and choosing it starts typing. From a box only the keys that don't type move,
// unless vim is in normal mode. The arrows at the top, and their keys, go back and on, and
// going on skips a question that isn't answered. Choosing one of several options goes on too.
// Nothing is sent until Send.

import { invoke } from "@tauri-apps/api/core";
import {
  type KeyboardEvent as ReactKeyboardEvent,
  type RefObject,
  useEffect,
  useRef,
  useState,
} from "react";
import { errorText, type FieldOption, type Question, type QuestionField, request } from "./engine";
import { act, isTextField, keysLabel, titled } from "./keybinds";
import { inPane } from "./keys";
import { Markdown } from "./markdown";
import { OPTION_ACTIONS } from "./PermissionCard";
import { answerValues, pagesOf, type Value } from "./questions";
import { Icon } from "./ui/Icon";
import { useVim } from "./VimBox";


/** One stop in a page's list. */
type Row =
  | { kind: "option"; option: FieldOption }
  | { kind: "box"; field: QuestionField }
  | { kind: "note" }
  | { kind: "next" }
  | { kind: "send" };

const NOTE: Row = { kind: "note" };
const NEXT: Row = { kind: "next" };
const SEND: Row = { kind: "send" };

export function QuestionCard({
  machineId,
  question,
  from,
  focusRef,
  takeFocus,
  onLeave,
}: {
  machineId: string;
  question: Question;
  /** The subagent asking, by name. */
  from?: string;
  /** What entering the question focuses, which the pane's compose key does. */
  focusRef: RefObject<HTMLDivElement | null>;
  /** Whether it takes the keyboard on arriving. Not from someone typing, whose next Enter
   * would answer it. */
  takeFocus: boolean;
  /** Going back out of it, to the transcript. */
  onLeave: () => void;
}) {
  const fields = question.fields ?? [];
  const pages = pagesOf(fields);
  const isUrl = question.url !== undefined;
  // The page for anything else comes after the questions. A page to open is all there is.
  const count = isUrl ? 1 : pages.length + 1;
  const [index, setIndex] = useState(0);
  const [values, setValues] = useState<Record<string, Value>>({});
  const [note, setNote] = useState("");
  const [highlight, setHighlight] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const boxes = useRef(new Map<string, HTMLTextAreaElement>());
  const onNote = !isUrl && index === pages.length;
  const page = onNote || isUrl ? undefined : pages[index];
  const main = page?.main ?? null;
  const options: FieldOption[] = main?.kind.type === "choice" ? main.kind.options : [];
  const multiple = main?.kind.type === "choice" && main.kind.multiple === true;

  const rows: Row[] = isUrl
    ? [SEND]
    : onNote
      ? [NOTE, SEND]
      : [
          ...options.map((option): Row => ({ kind: "option", option })),
          ...(page?.extra ?? []).map((field): Row => ({ kind: "box", field })),
          // Choosing one of several options goes on by itself. Anything else needs a way on
          // that doesn't mean knowing the arrows at the top.
          ...(options.length > 0 && !multiple ? [] : [NEXT]),
        ];
  const at = Math.min(highlight, Math.max(0, rows.length - 1));
  const row = rows[at];
  const boxKey = (target: Row | undefined) =>
    target?.kind === "box" ? target.field.key : target?.kind === "note" ? "note" : null;

  // The message box it replaced had the keyboard, and that went with it.
  useEffect(() => {
    if (takeFocus && document.activeElement === document.body) focusRef.current?.focus();
  }, [question.id, focusRef, takeFocus]);

  /** Moves to a row, out of a box if typing in one. A box moved onto is only highlighted. */
  const moveTo = (next: number) => {
    setHighlight(Math.max(0, Math.min(rows.length - 1, next)));
    if (document.activeElement instanceof HTMLTextAreaElement) focusRef.current?.focus();
  };

  /** Starts typing in a row's box. */
  const enter = (number: number) => {
    const key = boxKey(rows[number]);
    setHighlight(number);
    if (key) boxes.current.get(key)?.focus();
  };

  /** Shows a page, at the option it was answered with, if any. */
  const show = (next: number) => {
    if (next < 0 || next >= count) return;
    const shown = pages[next];
    const value = shown?.main ? values[shown.main.key] : undefined;
    const chosen =
      shown?.main?.kind.type === "choice"
        ? shown.main.kind.options.findIndex((option) =>
            Array.isArray(value) ? value[0] === option.value : value === option.value,
          )
        : -1;
    setIndex(next);
    setHighlight(Math.max(0, chosen));
    if (document.activeElement instanceof HTMLTextAreaElement) focusRef.current?.focus();
  };

  const send = async () => {
    setError(null);
    setSending(true);
    const text = note.trim();
    try {
      await request(machineId, {
        command: "answer_question",
        question_id: question.id,
        answer: {
          action: "accept",
          values: isUrl ? {} : answerValues(fields, values),
          ...(text && { note: text }),
        },
      });
    } catch (reason) {
      setError(errorText(reason));
      setSending(false);
    }
  };

  const set = (key: string, value: Value) => setValues((now) => ({ ...now, [key]: value }));

  const choose = (target: Row | undefined) => {
    switch (target?.kind) {
      case "option": {
        if (!main) return;
        const value = target.option.value;
        if (multiple) {
          const current = values[main.key];
          const list = Array.isArray(current) ? current : [];
          set(main.key, list.includes(value) ? list.filter((v) => v !== value) : [...list, value]);
        } else {
          set(main.key, value);
          show(index + 1);
        }
        return;
      }
      case "box": {
        const text = values[target.field.key];
        if (typeof text === "string" && text.trim() !== "") show(index + 1);
        return;
      }
      case "note":
        moveTo(rows.indexOf(SEND));
        return;
      case "next":
        show(index + 1);
        return;
      case "send":
        if (isUrl) {
          void invoke("open_url", { url: question.url ?? "" })
            .then(send)
            .catch((reason) => setError(errorText(reason)));
        } else {
          void send();
        }
        return;
    }
  };

  // Picking by number, as for permission requests. A key that types never does while you're
  // typing, so a box can take digits.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!inPane(event) || isTextField(event.target)) return;
      rows.some((target, number) => {
        const action = OPTION_ACTIONS[number];
        if (!action || (target.kind !== "option" && target.kind !== "box")) return false;
        return act(event, action, () => {
          if (target.kind === "box") enter(number);
          else {
            setHighlight(number);
            choose(target);
          }
        });
      });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  // The list's keys, while it has the keyboard. A box handles its own presses.
  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.target instanceof HTMLTextAreaElement) return;
    if (act(event, "question.up", () => moveTo(at - 1))) return;
    if (act(event, "question.down", () => moveTo(at + 1))) return;
    if (act(event, "question.prev", () => show(index - 1))) return;
    if (act(event, "question.next", () => show(index + 1))) return;
    if (act(event, "question.choose", () => (boxKey(row) ? enter(at) : choose(row)))) return;
    act(event, "question.leave", onLeave);
  };

  // One question on its own reads best as what the agent asked. With several, each one's own
  // wording says it, and the agent's message is only "answer these".
  const text = isUrl
    ? question.message
    : onNote
      ? null
      : (main?.description ?? (pages.length === 1 ? question.message : main?.title));
  const label = isUrl ? "Open a page" : onNote ? "Anything else?" : (main?.title ?? "Question");
  const box = (key: string, target: number) => ({
    boxRef: (element: HTMLTextAreaElement | null) => {
      if (element) boxes.current.set(key, element);
      else boxes.current.delete(key);
    },
    highlighted: target === at,
    onFocus: () => setHighlight(target),
    onMove: (step: 1 | -1) => moveTo(target + step),
    onLeave: () => focusRef.current?.focus(),
  });

  return (
    <div
      className="question"
      ref={focusRef}
      tabIndex={0}
      role="group"
      aria-label={text ?? label}
      onKeyDown={onKeyDown}
    >
      <div className="question-top">
        {count > 1 && (
          <button
            type="button"
            className="question-arrow"
            tabIndex={-1}
            aria-label="Previous question"
            title={titled("Previous question", "question.prev")}
            disabled={index === 0}
            onClick={() => show(index - 1)}
          >
            <Icon name="back" />
          </button>
        )}
        <span className="question-heading">{from ? `${from}: ${label}` : label}</span>
        {count > 1 && (
          <span className="question-count">
            {index + 1} of {count}
          </span>
        )}
        {count > 1 && (
          <button
            type="button"
            className="question-arrow"
            tabIndex={-1}
            aria-label="Next question"
            title={titled("Next question", "question.next")}
            disabled={index === count - 1}
            onClick={() => show(index + 1)}
          >
            <Icon name="unfold" />
          </button>
        )}
      </div>
      {text && (
        <div className="question-text">
          <Markdown text={text} />
        </div>
      )}
      {isUrl && <div className="question-url code">{question.url}</div>}
      {main?.kind.type === "boolean" && (
        <label className="question-row">
          <input
            type="checkbox"
            checked={values[main.key] === true}
            onChange={(event) => set(main.key, event.target.checked)}
          />
          <span>{main.title ?? "Yes"}</span>
        </label>
      )}
      {main?.kind.type === "number" && (
        <input
          className="question-row question-number-input"
          type="number"
          step={main.kind.integer ? 1 : undefined}
          value={typeof values[main.key] === "string" ? String(values[main.key]) : ""}
          aria-label={main.title ?? "Number"}
          onChange={(event) => set(main.key, event.target.value)}
        />
      )}
      {rows.length > 0 && (
        <div className="question-options" role={multiple ? "group" : "radiogroup"}>
          {rows.map((target, number) => {
            const shownNumber =
              number < OPTION_ACTIONS.length ? keysLabel(OPTION_ACTIONS[number]) : undefined;
            switch (target.kind) {
              case "option": {
                const value = main ? values[main.key] : undefined;
                const on = Array.isArray(value)
                  ? value.includes(target.option.value)
                  : value === target.option.value;
                return (
                  <button
                    key={target.option.value}
                    type="button"
                    // The list is one stop, reached through the question itself.
                    tabIndex={-1}
                    className="question-option"
                    data-highlighted={number === at ? "" : undefined}
                    role={multiple ? "checkbox" : "radio"}
                    aria-checked={on}
                    disabled={sending}
                    onMouseMove={() => setHighlight(number)}
                    onClick={() => {
                      setHighlight(number);
                      choose(target);
                    }}
                  >
                    <span className="question-number">
                      {multiple ? (
                        <span className={`check ${on ? "on" : ""}`}>
                          {on && <Icon name="check" />}
                        </span>
                      ) : (
                        shownNumber
                      )}
                    </span>
                    <span className="question-label">{target.option.label}</span>
                    {target.option.description && (
                      <span className="question-detail">{target.option.description}</span>
                    )}
                    {on && !multiple && <Icon name="check" />}
                  </button>
                );
              }
              case "box":
                return (
                  <AnswerBox
                    key={target.field.key}
                    {...box(target.field.key, number)}
                    number={shownNumber}
                    // Beside a question, a box is for an answer of your own, whatever the agent
                    // calls it.
                    placeholder={
                      main
                        ? "Type your own answer"
                        : (target.field.description ?? target.field.title ?? "")
                    }
                    label={target.field.title ?? "Your answer"}
                    value={
                      typeof values[target.field.key] === "string"
                        ? String(values[target.field.key])
                        : ""
                    }
                    onChange={(value) => set(target.field.key, value)}
                    onConfirm={() => choose(target)}
                  />
                );
              case "note":
                return (
                  <AnswerBox
                    key="note"
                    {...box("note", number)}
                    number={undefined}
                    placeholder="Add anything else the agent should know"
                    label="Anything else"
                    value={note}
                    onChange={setNote}
                    onConfirm={() => choose(target)}
                  />
                );
              case "next":
              case "send":
                return (
                  <button
                    key={target.kind}
                    type="button"
                    tabIndex={-1}
                    className="question-option question-send"
                    data-highlighted={number === at ? "" : undefined}
                    disabled={sending}
                    onMouseMove={() => setHighlight(number)}
                    onClick={() => choose(target)}
                  >
                    <span className="question-number">
                      <Icon name={target.kind === "next" ? "unfold" : "send"} />
                    </span>
                    <span className="question-label">
                      {target.kind === "next" ? "Next" : isUrl ? "Open the page" : "Send"}
                    </span>
                  </button>
                );
            }
          })}
        </div>
      )}
      {error && <p className="error">{error}</p>}
    </div>
  );
}

/** A box in the list that takes typing, with vim if it's on. Typing keys type, and the list's
 * other keys still move. In vim's normal mode the letter keys move too, since they don't type
 * there. */
function AnswerBox({
  boxRef,
  number,
  highlighted,
  placeholder,
  label,
  value,
  onChange,
  onFocus,
  onMove,
  onConfirm,
  onLeave,
}: {
  boxRef: (box: HTMLTextAreaElement | null) => void;
  number: string | undefined;
  highlighted: boolean;
  placeholder: string;
  label: string;
  value: string;
  onChange: (value: string) => void;
  onFocus: () => void;
  onMove: (step: 1 | -1) => void;
  onConfirm: () => void;
  onLeave: () => void;
}) {
  const box = useRef<HTMLTextAreaElement | null>(null);
  const vim = useVim(box);
  return (
    <label className="question-option" data-highlighted={highlighted ? "" : undefined}>
      {/* Only a box beside options lines up with their numbers. */}
      {number !== undefined && <span className="question-number">{number}</span>}
      <textarea
        ref={(element) => {
          box.current = element;
          boxRef(element);
        }}
        className="question-input"
        rows={1}
        tabIndex={-1}
        value={value}
        placeholder={placeholder}
        aria-label={label}
        autoCorrect="off"
        autoCapitalize="off"
        {...vim.props}
        onFocus={() => {
          vim.props.onFocus();
          onFocus();
        }}
        onChange={(event) => onChange(event.target.value.replace(/\n/g, " "))}
        onKeyDown={(event) => {
          const typing = vim.shown === null || vim.shown === "insert";
          if (act(event, "question.up", () => onMove(-1), { typing })) return;
          if (act(event, "question.down", () => onMove(1), { typing })) return;
          if (act(event, "question.choose", onConfirm, { typing: true })) return;
          if (vim.onKeyDown(event)) return;
          act(event, "question.leave", onLeave, { typing: true });
        }}
      />
    </label>
  );
}
