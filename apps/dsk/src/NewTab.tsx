import { useEffect, useRef } from "react";
import { type AgentChoice, type AgentDraft, AgentForm } from "./AgentForm";
import { errorText, request } from "./engine";
import { act, type ActionId } from "./keybinds";
import { focusPane, inPane } from "./keys";
import type { TopicEntry } from "./model";
import { hint, type Hint, useHints } from "./shortcuts";
import { Icon, type IconName } from "./ui/Icon";
import { Keys } from "./ui/StatusIcon";

export type TabKind = "agent" | "terminal" | "git" | "diff";

/** What this client has done in a topic's new tab: the kind chosen, if any, and the draft for
 * an agent, which stays when you go back to choosing. */
export interface NewTabState {
  kind: TabKind | null;
  agent: AgentDraft;
}

export const NEW_TAB_STATE: NewTabState = { kind: null, agent: {} };

interface Kind {
  kind: TabKind;
  label: string;
  icon: IconName;
  action: ActionId;
  built: boolean;
}

// In the order of their keys, so the list reads as a numbered menu.
const KINDS: Kind[] = [
  { kind: "agent", label: "Agent", icon: "agent", action: "new_tab.agent", built: true },
  { kind: "terminal", label: "Terminal", icon: "terminal", action: "new_tab.terminal", built: false },
  { kind: "git", label: "Git", icon: "git", action: "new_tab.git", built: false },
  { kind: "diff", label: "Diff", icon: "diff", action: "new_tab.diff", built: false },
];

const BUILT = KINDS.filter((k) => k.built);

/** Where a topic's next tab starts: choose what it is, then set it up. Choosing is a list
 * rather than a form, so every kind is one key away and kinds that need no setup can open
 * straight away. */
export function NewTab({
  entry,
  state,
  onState,
  onSettings,
  onStarted,
  onStalled,
}: {
  entry: TopicEntry;
  state: NewTabState;
  onState: (state: NewTabState) => void;
  onSettings: (machineId: string) => void;
  onStarted: (sessionId: string) => void;
  /** The agent started but its prompt didn't reach it, so starting again would make another. */
  onStalled: (sessionId: string, reason: string) => void;
}) {
  const { machineId, topic } = entry;

  const choose = (kind: TabKind) => {
    if (!KINDS.some((k) => k.kind === kind && k.built)) return;
    onState({ ...state, kind });
    focusPane();
  };

  const start = async (choice: AgentChoice) => {
    const started = await request(machineId, {
      command: "start_session",
      topic_id: topic.id,
      agent: choice.agent,
      mode: choice.mode,
      model: choice.model,
      effort: choice.effort,
    });
    if (started.output !== "session") throw new Error("The daemon didn't start an agent");
    try {
      await request(machineId, {
        command: "send_prompt",
        session_id: started.session.id,
        text: choice.text,
      });
      onStarted(started.session.id);
    } catch (reason) {
      onStalled(started.session.id, errorText(reason));
    }
  };

  if (state.kind === "agent") {
    return (
      <AgentForm
        device={entry.device}
        draft={state.agent}
        onDraft={(change) => onState({ ...state, agent: { ...state.agent, ...change } })}
        onStart={start}
        onCancel={() => {
          onState({ ...state, kind: null });
          focusPane();
        }}
        onSettings={onSettings}
      />
    );
  }
  return <Kinds onChoose={choose} />;
}

function Kinds({ onChoose }: { onChoose: (kind: TabKind) => void }) {
  const list = useRef<HTMLDivElement>(null);
  // Read through a ref, because the listener is added once.
  const chooseRef = useRef(onChoose);
  chooseRef.current = onChoose;

  const hints: Hint[] = BUILT.map((k) => hint(k.label, k.action));
  if (BUILT.length > 1) hints.push(hint("Move", "new_tab.up", "new_tab.down"));
  hints.push(hint("Sidebar", "pane.back"));
  useHints("pane", hints);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!inPane(event)) return;
      const choices = [...(list.current?.querySelectorAll("button:not(:disabled)") ?? [])];
      const focused = choices.findIndex((button) => button === document.activeElement);
      const move = (step: number) => {
        const from = focused === -1 && step < 0 ? 0 : focused;
        const next = choices[(from + step + choices.length) % choices.length];
        if (next instanceof HTMLElement) next.focus();
      };
      const handled = KINDS.some(({ kind, action }) =>
        act(event, action, () => chooseRef.current(kind)),
      );
      if (handled) return;
      if (act(event, "new_tab.up", () => move(-1))) return;
      if (act(event, "new_tab.down", () => move(1))) return;
      act(event, "new_tab.confirm", () => {
        const chosen = choices[focused === -1 ? 0 : focused];
        if (chosen instanceof HTMLElement) chosen.click();
      });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div className="pane create">
      <div ref={list} className="new-tab" role="menu" aria-label="New tab">
        {KINDS.map((k) => (
          <button
            key={k.kind}
            type="button"
            role="menuitem"
            className="new-tab-kind"
            disabled={!k.built}
            onClick={() => onChoose(k.kind)}
          >
            <Icon name={k.icon} />
            <span className="name">{k.label}</span>
            {!k.built && <span className="meta">Not built yet</span>}
            <Keys action={k.action} />
          </button>
        ))}
      </div>
    </div>
  );
}
