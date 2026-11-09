import { type FocusEvent, type KeyboardEvent, useEffect, useId, useRef, useState } from "react";
import type { AgentCommand } from "./engine";
import { act } from "./keybinds";
import { moveHighlight } from "./pathInput";
import { hint, type Hint } from "./shortcuts";
import { commandQuery, completedText, matchCommands, sendsAtOnce } from "./slashCommands";
import type { VimMode } from "./vim";

interface FocusProps {
  onFocus(): void;
  onBlur(event: FocusEvent<HTMLTextAreaElement>): void;
}

const HINTS: Hint[] = [
  hint("Move", "composer.command_up", "composer.command_down"),
  hint("Complete", "composer.complete_command"),
];

/** Suggests the agent's slash commands in the message box while it holds only `/` and part of
 * a name. The box keeps the keyboard throughout. */
export function useCommandSuggestions({
  text,
  commands,
  vimMode,
  onChange,
  onSend,
}: {
  text: string;
  commands: AgentCommand[];
  /** The box's vim mode. In normal mode the keys are vim's, so the list waits for insert mode. */
  vimMode: VimMode | null;
  onChange: (text: string) => void;
  onSend: (text: string) => void;
}) {
  const [focused, setFocused] = useState(false);
  // Which suggestion is chosen, by the text it was chosen for, so typing on starts over at the
  // first. Null is a stop of its own, for sending what's typed as it is.
  const [chosen, setChosen] = useState<{ text: string; index: number | null }>({
    text: "",
    index: 0,
  });
  const listId = useId();

  const query = commandQuery(text);
  const suggesting = query !== null && focused && (vimMode === null || vimMode === "insert");
  const suggestions = suggesting ? matchCommands(commands, query) : [];
  const highlight = chosen.text === text ? chosen.index : 0;
  const suggested = highlight === null ? undefined : suggestions[highlight];
  const highlightRow = (index: number | null) => setChosen({ text, index });

  const choose = (command: AgentCommand) => {
    if (sendsAtOnce(command)) onSend(`/${command.name}`);
    else onChange(completedText(command));
  };

  return {
    hints: suggestions.length > 0 ? HINTS : [],
    /** Goes after the box's own props, and passes focus on to `inner`, such as vim's. */
    boxProps: (inner: FocusProps) => ({
      role: "combobox",
      "aria-autocomplete": "list" as const,
      "aria-expanded": suggestions.length > 0,
      "aria-controls": listId,
      "aria-activedescendant": suggested ? `${listId}-${highlight}` : undefined,
      onFocus: () => {
        inner.onFocus();
        setFocused(true);
      },
      onBlur: (event: FocusEvent<HTMLTextAreaElement>) => {
        inner.onBlur(event);
        setFocused(false);
      },
    }),
    /** Moving and completing. Says whether it took the press. */
    onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>): boolean {
      if (suggestions.length === 0 || event.nativeEvent.isComposing) return false;
      const move = (step: 1 | -1) =>
        highlightRow(moveHighlight(highlight, step, suggestions.length));
      if (act(event, "composer.command_up", () => move(-1))) return true;
      if (act(event, "composer.command_down", () => move(1))) return true;
      return act(event, "composer.complete_command", () => {
        const command = suggested ?? suggestions[0];
        if (command) onChange(completedText(command));
      });
    },
    /** For the box's send key: runs the chosen command, and says whether there was one. */
    sendChosen(): boolean {
      if (!suggested) return false;
      choose(suggested);
      return true;
    },
    list:
      suggestions.length > 0 ? (
        <CommandList
          id={listId}
          commands={suggestions}
          highlight={highlight}
          onHighlight={highlightRow}
          onChoose={choose}
        />
      ) : null,
  };
}

function CommandList({
  id,
  commands,
  highlight,
  onHighlight,
  onChoose,
}: {
  id: string;
  commands: AgentCommand[];
  highlight: number | null;
  onHighlight: (index: number) => void;
  onChoose: (command: AgentCommand) => void;
}) {
  const list = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (highlight !== null) list.current?.children[highlight]?.scrollIntoView({ block: "nearest" });
  }, [highlight]);
  return (
    <div ref={list} id={id} className="command-list palette-list" role="listbox">
      {commands.map((command, index) => (
        <div
          key={command.name}
          id={`${id}-${index}`}
          className="palette-item"
          role="option"
          aria-selected={index === highlight}
          data-highlighted={index === highlight ? "" : undefined}
          // The text box keeps the keyboard, so typing carries on after pointing at one.
          onMouseDown={(event) => event.preventDefault()}
          onMouseMove={() => onHighlight(index)}
          onClick={() => onChoose(command)}
        >
          <span className="command-name code">/{command.name}</span>
          {command.hint && <span className="command-hint code">{command.hint}</span>}
          <span className="name">{command.description}</span>
        </div>
      ))}
    </div>
  );
}
