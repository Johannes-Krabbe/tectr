import { useEffect, useRef, useState } from "react";
import { type Conversation, errorText, request } from "./engine";
import { act } from "./keybinds";
import { conversationsFor, folderName, type TopicEntry } from "./model";
import { Icon } from "./ui/Icon";

const shortDate = (ms: number) =>
  new Date(ms).toLocaleDateString(undefined, { month: "short", day: "numeric" });

/** Every typed word has to appear in the title, the folder or the agent. */
function matches(conversation: Conversation, agentName: string, query: string): boolean {
  const text = `${conversation.title} ${conversation.cwd} ${agentName}`.toLowerCase();
  return query
    .toLowerCase()
    .split(/\s+/)
    .every((word) => text.includes(word));
}

/** A new tab's way to carry on a Claude Code or Codex conversation here. It's the tab's own
 * page, like starting an agent: a search box over the conversations on the topic's device, the
 * ones from this workspace first. Typing filters, the list keys move, and Enter or a click
 * imports. Esc goes back to choosing what the tab is. */
export function ImportConversation({
  entry,
  onImport,
  onBack,
}: {
  entry: TopicEntry;
  onImport: (agent: string, id: string) => void;
  onBack: () => void;
}) {
  const { machineId, topic, workspace } = entry;
  const providers = entry.device.state?.providers ?? [];
  const [found, setFound] = useState<Conversation[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [highlight, setHighlight] = useState(0);
  const list = useRef<HTMLDivElement>(null);

  // They're in each agent's own files on the device, so it's asked whenever this opens.
  useEffect(() => {
    let shown = true;
    request(machineId, { command: "list_conversations" })
      .then((output) => {
        if (!shown) return;
        const listed = output.output === "conversations" ? output.conversations : [];
        setFound(conversationsFor(listed, workspace, topic));
      })
      .catch((reason) => {
        if (shown) setError(errorText(reason));
      });
    return () => {
      shown = false;
    };
  }, [machineId]);

  const agentName = (agent: string) => providers.find((p) => p.agent === agent)?.name ?? agent;
  const shown = (found ?? []).filter((c) => matches(c, agentName(c.agent), query));
  const current = Math.max(Math.min(highlight, shown.length - 1), 0);

  useEffect(() => {
    list.current?.querySelector("[data-highlighted]")?.scrollIntoView({ block: "nearest" });
  }, [current, shown.length]);

  const move = (step: 1 | -1) => () =>
    setHighlight(shown.length === 0 ? 0 : (current + step + shown.length) % shown.length);
  const choose = (conversation: Conversation | undefined) => {
    if (conversation) onImport(conversation.agent, conversation.id);
  };

  return (
    <div className="pane import">
      <div className="import-in">
        <div className="import-head">
          <button className="icon-button" title="Back" aria-label="Back to the new tab" onClick={onBack}>
            <Icon name="back" />
          </button>
          <h2 className="import-title">Import a conversation</h2>
        </div>
        <input
          className="import-search"
          autoFocus
          value={query}
          placeholder="Search Claude Code and Codex conversations"
          aria-label="Search conversations"
          spellCheck={false}
          autoCorrect="off"
          autoCapitalize="off"
          onChange={(event) => {
            setQuery(event.target.value);
            setHighlight(0);
          }}
          onKeyDown={(event) => {
            if (event.nativeEvent.isComposing) return;
            if (act(event, "picker.up", move(-1))) return;
            if (act(event, "picker.down", move(1))) return;
            if (act(event, "picker.confirm", () => choose(shown[current]))) return;
            act(event, "picker.cancel", () => {
              if (query) setQuery("");
              else onBack();
            });
          }}
        />
        {error && <p className="error">{error}</p>}
        {!found && !error && <p className="import-note">Looking for conversations</p>}
        {found && shown.length === 0 && (
          <p className="import-note">
            {found.length === 0 ? "No Claude Code or Codex conversations to import" : "Nothing matches"}
          </p>
        )}
        <div ref={list} className="import-list" role="listbox" aria-label="Conversations">
          {shown.map((conversation, index) => (
            <button
              key={`${conversation.agent}/${conversation.id}`}
              className="list-choice"
              role="option"
              // The search box keeps the keyboard, and its keys move through these.
              tabIndex={-1}
              aria-selected={index === current}
              data-highlighted={index === current ? "" : undefined}
              onMouseMove={() => setHighlight(index)}
              onClick={() => choose(conversation)}
            >
              <span className="name">{conversation.title}</span>
              <span className="meta">
                {[agentName(conversation.agent), folderName(conversation.cwd), shortDate(conversation.updated_at_ms)].join(
                  ", ",
                )}
              </span>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
