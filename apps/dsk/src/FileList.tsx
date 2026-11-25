import { type FocusEvent, type KeyboardEvent, useEffect, useId, useRef, useState } from "react";
import { request } from "./engine";
import { act } from "./keybinds";
import { completeMention, mentionAt } from "./mentions";
import { moveHighlight } from "./pathInput";
import type { VimMode } from "./vim";

interface FocusProps {
  onFocus(): void;
  onBlur(event: FocusEvent<HTMLTextAreaElement>): void;
}

/** How long typing has to pause before the daemon is asked, so each key doesn't ask. */
const ASK_AFTER_MS = 60;

/** Offers the session folder's files while the caret is in an `@` mention, with the same keys
 * as the slash command list. The box keeps the keyboard throughout. */
export function useFileSuggestions({
  machineId,
  sessionId,
  text,
  caret,
  vimMode,
  onChange,
}: {
  machineId: string;
  sessionId: string;
  text: string;
  caret: number;
  vimMode: VimMode | null;
  /** The text with the mention completed, and where the caret goes. */
  onChange: (text: string, caret: number) => void;
}) {
  const [focused, setFocused] = useState(false);
  const [found, setFound] = useState<{ query: string; paths: string[] }>({ query: "", paths: [] });
  const [chosen, setChosen] = useState<{ query: string | null; index: number }>({
    query: null,
    index: 0,
  });
  const listId = useId();

  const mention = mentionAt(text, caret);
  const active = mention !== null && focused && (vimMode === null || vimMode === "insert");
  const query = mention?.query ?? null;

  useEffect(() => {
    if (!active || query === null) return;
    let current = true;
    const timer = setTimeout(() => {
      request(machineId, { command: "list_files", session_id: sessionId, query })
        .then((answer) => {
          if (current && answer.output === "files") setFound({ query, paths: answer.paths });
        })
        .catch(() => {});
    }, ASK_AFTER_MS);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [active, query, machineId, sessionId]);

  const paths = active && found.query === query ? found.paths : [];
  const highlight = chosen.query === query ? Math.min(chosen.index, paths.length - 1) : 0;

  const choose = (path: string) => {
    if (!mention) return;
    const done = completeMention(text, caret, mention, path);
    onChange(done.text, done.caret);
  };

  return {
    /** Passes focus on to `inner`. */
    focusProps: (inner: FocusProps): FocusProps => ({
      onFocus: () => {
        inner.onFocus();
        setFocused(true);
      },
      onBlur: (event) => {
        inner.onBlur(event);
        setFocused(false);
      },
    }),
    /** Moving and completing. Says whether it took the press. */
    onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>): boolean {
      if (paths.length === 0 || event.nativeEvent.isComposing) return false;
      const move = (step: 1 | -1) =>
        setChosen({ query, index: moveHighlight(highlight, step, paths.length) ?? 0 });
      if (act(event, "composer.command_up", () => move(-1))) return true;
      if (act(event, "composer.command_down", () => move(1))) return true;
      return act(event, "composer.complete_command", () => choose(paths[highlight]));
    },
    /** For the box's send key: completes the chosen file, and says whether there was one. */
    sendChosen(): boolean {
      const path = paths[highlight];
      if (path === undefined) return false;
      choose(path);
      return true;
    },
    list:
      paths.length > 0 ? (
        <FileChoices
          id={listId}
          paths={paths}
          highlight={highlight}
          onHighlight={(index) => setChosen({ query, index })}
          onChoose={choose}
        />
      ) : null,
  };
}

function FileChoices({
  id,
  paths,
  highlight,
  onHighlight,
  onChoose,
}: {
  id: string;
  paths: string[];
  highlight: number;
  onHighlight: (index: number) => void;
  onChoose: (path: string) => void;
}) {
  const list = useRef<HTMLDivElement>(null);
  useEffect(() => {
    list.current?.children[highlight]?.scrollIntoView({ block: "nearest" });
  }, [highlight]);
  return (
    <div ref={list} id={id} className="command-list list-choices" role="listbox" aria-label="Files">
      {paths.map((path, index) => {
        const slash = path.lastIndexOf("/");
        return (
          <div
            key={path}
            className="list-choice"
            role="option"
            aria-selected={index === highlight}
            data-highlighted={index === highlight ? "" : undefined}
            // The text box keeps the keyboard, so typing carries on after pointing at one.
            onMouseDown={(event) => event.preventDefault()}
            onMouseMove={() => onHighlight(index)}
            onClick={() => onChoose(path)}
          >
            <span className="command-name code">{path.slice(slash + 1)}</span>
            {slash > 0 && <span className="name code">{path.slice(0, slash)}</span>}
          </div>
        );
      })}
    </div>
  );
}
