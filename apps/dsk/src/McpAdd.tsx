import { Fragment, useRef, useState } from "react";
import { type CommandOutput, errorText, type McpServer, request } from "./engine";
import {
  blankId,
  blanksReady,
  filledIn,
  type Found,
  freeName,
  mcpSummary,
  nameProblem,
  readPasted,
} from "./mcp";
import { Sheet } from "./Sheets";

const ASK = "A URL, a command, a claude or codex mcp add line, or JSON from the docs";

/** Adds servers to the library from whatever was pasted. The full form is only for changing a
 * server later. `taken` are the names these servers can't have. `onAdded` gets them as saved,
 * which says whether each wants a login, once they all are. */
export function McpAddSheet({
  machineId,
  taken,
  where,
  onAdded,
  onClose,
}: {
  machineId: string;
  taken: Set<string>;
  where: string;
  onAdded?: (servers: McpServer[]) => void;
  onClose: () => void;
}) {
  const saved = useRef<McpServer[]>([]);
  const [text, setText] = useState("");
  // Kept by position: what was pasted decides the servers, and these only adjust them.
  const [names, setNames] = useState<Record<number, string>>({});
  const [values, setValues] = useState<Record<string, string>>({});

  const pasted = readPasted(text);
  const found: Found[] = "found" in pasted ? pasted.found : [];
  const used = new Set(taken);
  const named = found.map((server, index) => {
    const name = names[index] ?? freeName(server.name, used);
    used.add(name);
    return name;
  });
  const valueOf = (index: number, id: string) => values[`${index}/${id}`] ?? "";
  const valuesFor = (index: number) =>
    Object.fromEntries(
      found[index].blanks.map((blank) => [blankId(blank), valueOf(index, blankId(blank))]),
    );

  const nameIssue = named
    .map((name, index) => {
      const problem = nameProblem(name.trim());
      if (problem) return problem;
      if (taken.has(name.trim())) return `There's already a server called ${name.trim()}`;
      return named.indexOf(name) !== index ? `Two servers are called ${name}` : null;
    })
    .find((problem) => problem !== null);
  const ready =
    found.length > 0 &&
    nameIssue === undefined &&
    found.every((server, index) => blanksReady(server, valuesFor(index)));

  const hint =
    "error" in pasted
      ? pasted.error
      : (nameIssue ?? (found.length === 0 ? ASK : undefined));

  const add = async (): Promise<CommandOutput> => {
    let last: CommandOutput = { output: "done" };
    const added: string[] = [];
    saved.current = [];
    for (const [index, server] of found.entries()) {
      try {
        last = await request(machineId, {
          command: "save_mcp_server",
          name: named[index].trim(),
          transport: filledIn(server, valuesFor(index)),
        });
      } catch (reason) {
        const before = added.length ? `Added ${added.join(", ")}, but ` : "";
        throw `${before}${named[index]}: ${errorText(reason)}`;
      }
      added.push(named[index]);
      if (last.output === "mcp_server") saved.current.push(last.server);
    }
    return last;
  };

  return (
    <Sheet
      where={where}
      action={found.length > 1 ? `Add ${found.length}` : "Add"}
      hint={hint}
      canSubmit={ready}
      returnsFocus
      run={add}
      onClose={onClose}
      onDone={() => onAdded?.(saved.current)}
      input={text}
    >
      <textarea
        className="sheet-name code"
        autoFocus
        rows={1}
        value={text}
        placeholder="Paste an MCP server"
        aria-label="MCP server to add"
        spellCheck={false}
        autoCorrect="off"
        autoCapitalize="off"
        onChange={(event) => setText(event.target.value)}
      />
      {found.length > 0 && (
        <div className="sheet-fields">
          {found.map((server, index) => (
            <Fragment key={index}>
              <label className="sheet-field">
                <span className="sheet-label">
                  {server.transport.type === "http" ? "HTTP" : "Stdio"}
                </span>
                <input
                  className="sheet-input code"
                  value={named[index]}
                  aria-label="Name"
                  spellCheck={false}
                  autoCorrect="off"
                  autoCapitalize="off"
                  onChange={(event) => setNames((all) => ({ ...all, [index]: event.target.value }))}
                />
              </label>
              <span className="sheet-field">
                <span className="sheet-label" />
                <span className="server-summary">{mcpSummary(server.transport)}</span>
              </span>
              {server.blanks.map((blank) => {
                const id = blankId(blank);
                return (
                  <label key={id} className="sheet-field">
                    <span className="sheet-label">{blank.key}</span>
                    <input
                      className="sheet-input code"
                      value={valueOf(index, id)}
                      // Empty reads the variable from the daemon's environment.
                      placeholder={blank.in === "env" ? `\${${blank.key}}` : "Required"}
                      spellCheck={false}
                      autoCorrect="off"
                      autoCapitalize="off"
                      onChange={(event) =>
                        setValues((all) => ({ ...all, [`${index}/${id}`]: event.target.value }))
                      }
                    />
                  </label>
                );
              })}
            </Fragment>
          ))}
        </div>
      )}
    </Sheet>
  );
}
