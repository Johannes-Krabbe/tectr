// The agent's unfinished todo list, above the message box. It's what you watch while the
// agent works, so it stays in sight there until it's all done, and folds to one line by hand.
// Subagents are steps where they started, and background tasks are in the box's foot.

import { useState } from "react";
import type { PlanEntry } from "./engine";
import { type StatusState, todoSummary } from "./model";
import { Icon } from "./ui/Icon";
import { StatusIcon } from "./ui/StatusIcon";

const ENTRY_STATE: Record<PlanEntry["status"], StatusState> = {
  pending: "idle",
  in_progress: "working",
  completed: "read",
  unknown: "idle",
};

export function WorkPanel({ plan }: { plan: PlanEntry[] | null }) {
  const summary = todoSummary(plan);
  const [open, setOpen] = useState(true);
  if (!summary || !plan) return null;
  return (
    <div className="work">
      <button
        type="button"
        className="activity-line work-line"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        <Icon name={open ? "fold" : "unfold"} />
        {summary}
      </button>
      {open && (
        <ul className="work-in" aria-label="Todo list">
          {plan.map((entry, index) => (
            <li key={index} className={`work-row ${entry.status === "completed" ? "done" : ""}`}>
              <StatusIcon state={ENTRY_STATE[entry.status]} />
              <span className="work-name">{entry.content}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
