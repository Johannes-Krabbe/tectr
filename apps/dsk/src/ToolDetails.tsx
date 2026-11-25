// A tool call's input and output, under its step or in a request for permission to run it.
// Code reads as a band rather than a box: a command, an edit or an output, highlighted the way
// the diff tab highlights, with a column for the sign of each line. A request lets its bands run
// edge to edge, so nothing sits in a box inside the card.

import { useMemo, useState } from "react";
import { languageOf } from "./diff";
import type { ToolCallPatch } from "./engine";
import type { SyntaxSpan } from "./markdown";
import { Markdown } from "./markdown";
import { type DiffLine, inputSummary, lineDiff, relativePath, SHOWN_OUTPUT_CHARS } from "./toolView";
import { useHighlight } from "./useHighlight";

export function ToolDetails({ tool, folder }: { tool: ToolCallPatch; folder?: string }) {
  const input = inputSummary(tool);
  const content = (tool.content ?? []).filter(
    (item) => (item.type === "text" && item.text.trim() !== "") || item.type === "diff",
  );
  const diffs = content.filter((item) => item.type === "diff");
  // An edit's diff already names its file.
  const diffed = diffs.some((item) => item.path === input?.text);
  return (
    <div className="tool-details">
      {input?.kind === "command" && <CommandBand command={input.text} />}
      {input?.kind === "path" && !diffed && (
        <div className="tool-input code">{relativePath(input.text, folder)}</div>
      )}
      {input?.kind === "json" && <pre className="tool-band tool-text">{input.text}</pre>}
      {input?.kind === "markdown" && (
        <div className="tool-band tool-markdown">
          <Markdown text={input.text} />
        </div>
      )}
      {content.map((item, index) =>
        item.type === "text" ? (
          <ToolOutput key={index} text={item.text} />
        ) : item.type === "diff" ? (
          <ToolDiff
            key={index}
            // One file is named by the step or the request above it.
            label={diffs.length > 1 ? relativePath(item.path, folder) : undefined}
            path={item.path}
            oldText={item.old_text}
            newText={item.new_text}
          />
        ) : null,
      )}
    </div>
  );
}

function ToolOutput({ text }: { text: string }) {
  const [all, setAll] = useState(false);
  const long = text.length > SHOWN_OUTPUT_CHARS;
  return (
    <>
      <pre className="tool-band tool-text">
        {all || !long ? text : `${text.slice(0, SHOWN_OUTPUT_CHARS)}...`}
      </pre>
      {long && (
        <button type="button" className="link-button" onClick={() => setAll(!all)}>
          {all ? "Show less" : `Show all ${text.length.toLocaleString()} characters`}
        </button>
      )}
    </>
  );
}

function CommandBand({ command }: { command: string }) {
  const lines = useMemo(() => [command], [command]);
  const spans = useHighlight(lines, "sh");
  return (
    <div className="tool-band">
      <div className="tool-line">
        <span className="tool-sign">$</span>
        <Code text={command} spans={spans?.flat() ?? null} />
      </div>
    </div>
  );
}

const SIGN: Record<Exclude<DiffLine["kind"], "gap">, string> = {
  added: "+",
  removed: "-",
  context: "",
};

function ToolDiff({
  label,
  path,
  oldText,
  newText,
}: {
  label: string | undefined;
  path: string;
  oldText: string | null;
  newText: string;
}) {
  const lines = useMemo(() => lineDiff(oldText, newText), [oldText, newText]);
  // Both sides' lines in one go, which reads right for everything but a change inside a block
  // comment or a long string.
  const texts = useMemo(
    () => lines.flatMap((line) => (line.kind === "gap" ? [] : [line.text])),
    [lines],
  );
  const spans = useHighlight(texts, languageOf(path));
  let at = 0;
  return (
    <>
      {label && <div className="tool-input code">{label}</div>}
      <div className="tool-band">
        {lines.map((line, index) => {
          if (line.kind === "gap") {
            return (
              <div key={index} className="tool-gap">
                {line.count === 1 ? "1 unchanged line" : `${line.count} unchanged lines`}
              </div>
            );
          }
          const lineSpans = spans?.[at++] ?? null;
          return (
            <div key={index} className={`tool-line diff-line ${line.kind}`}>
              <span className="tool-sign">{SIGN[line.kind]}</span>
              <Code text={line.text} spans={lineSpans} />
            </div>
          );
        })}
      </div>
    </>
  );
}

function Code({ text, spans }: { text: string; spans: SyntaxSpan[] | null }) {
  // A space keeps an empty line its height.
  if (!spans || spans.length === 0) return <span className="tool-code">{text || " "}</span>;
  return (
    <span className="tool-code">
      {spans.map((span, index) => (
        <span key={index} className={`syntax-${span.role}`}>
          {span.text}
        </span>
      ))}
    </span>
  );
}
