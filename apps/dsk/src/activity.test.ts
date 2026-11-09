import assert from "node:assert/strict";
import { test } from "node:test";
import type { Session, SessionEvent, SessionEventKind } from "./engine";
import {
  type Activity,
  activitySummary,
  foldState,
  foldTranscript,
  formatWorked,
  liveStepLabel,
  NO_FOLDS,
  transcriptTiming,
  type TranscriptItem,
} from "./model.ts";

/** Events one second apart, unless one says when it happened. */
function events(...kinds: (SessionEventKind | [number, SessionEventKind])[]): SessionEvent[] {
  return kinds.map((entry, index) => {
    const [at, kind] = Array.isArray(entry) ? entry : [(index + 1) * 1000, entry];
    return { seq: index + 1, at_ms: at, kind };
  });
}

const prompt = (text: string): SessionEventKind => ({ event: "user_prompt", text });
const said = (text: string): SessionEventKind => ({ event: "agent_message", text });
const thought = (text: string): SessionEventKind => ({ event: "agent_thought", text });
const tool = (id: string, title: string): SessionEventKind => ({
  event: "tool_call",
  id,
  tool: { title, status: "in_progress" },
});
const finished = (id: string, status: "completed" | "failed" = "completed"): SessionEventKind => ({
  event: "tool_call_update",
  id,
  tool: { status },
});

const activities = (items: TranscriptItem[]) =>
  items.filter((item): item is Activity => item.kind === "activity");
const kinds = (items: TranscriptItem[]) => items.map((item) => item.kind);
const session = (status: Session["status"]) => ({ status }) as Session;

test("messages between tool calls stay, and each run of tools folds on its own", () => {
  const items = foldTranscript(
    events(
      prompt("fix it"),
      tool("a", "Read a.rs"),
      tool("b", "Read b.rs"),
      said("Found it. "),
      said("Editing now."),
      tool("c", "Edit a.rs"),
      said("Done."),
    ),
  );
  assert.deepEqual(kinds(items), ["prompt", "activity", "message", "activity", "message"]);
  assert.equal(items[2].kind === "message" && items[2].text, "Found it. Editing now.");
  assert.deepEqual(
    activities(items).map((activity) => activity.steps.length),
    [2, 1],
  );
});

test("thinking joins the activity around it, and streamed thoughts become one step", () => {
  const [, activity] = foldTranscript(
    events(prompt("hi"), thought("Let me "), thought("look."), tool("a", "Read"), thought("Ok.")),
  );
  assert.equal(activity.kind, "activity");
  assert.deepEqual(
    activity.kind === "activity" && activity.steps.map((step) => step.kind),
    ["thought", "tool", "thought"],
  );
});

test("a tool's updates land on its step, even after a message closed its activity", () => {
  const items = foldTranscript(
    events(prompt("hi"), tool("a", "Slow"), said("Still going."), finished("a", "failed")),
  );
  assert.deepEqual(kinds(items), ["prompt", "activity", "message"]);
  const [activity] = activities(items);
  const [step] = activity.steps;
  assert.equal(step.kind === "tool" && step.tool.status, "failed");
  assert.equal(step.kind === "tool" && step.tool.title, "Slow");
});

test("answers to requests fold with the tools, and errors and stops never do", () => {
  const approval = {
    id: "r",
    session_id: "s",
    tool_call: { title: "rm -rf" },
    options: [{ id: "no", name: "Deny", kind: "reject_once" as const }],
  };
  const items = foldTranscript(
    events(
      tool("a", "Read"),
      { event: "permission_requested", approval },
      {
        event: "permission_resolved",
        approval_id: "r",
        outcome: { outcome: "selected", option_id: "no" },
      },
      { event: "error", message: "boom" },
      tool("b", "Read"),
      { event: "turn_ended", stop_reason: "cancelled" },
    ),
  );
  assert.deepEqual(kinds(items), ["activity", "note", "activity", "note"]);
  const [first] = activities(items);
  assert.deepEqual(first.steps[1], { kind: "note", key: 3, text: "Request: rm -rf. Deny" });
});

test("time worked runs from going quiet to speaking again, without time waiting on you", () => {
  const approval = { id: "r", session_id: "s", tool_call: {}, options: [] };
  const [, activity] = foldTranscript(
    events(
      [1_000, prompt("hi")],
      [3_000, tool("a", "Read")],
      [4_000, { event: "permission_requested", approval }],
      [64_000, { event: "permission_resolved", approval_id: "r", outcome: { outcome: "cancelled" } }],
      [66_000, said("Done")],
    ),
  );
  assert.equal(activity.kind === "activity" && activitySummary(activity).text, "Worked for 5s, 1 step");
});

test("the summary counts steps and failures, and thinking alone says so", () => {
  const summary = (...kinds: SessionEventKind[]) => {
    const [activity] = activities(foldTranscript(events(prompt("hi"), ...kinds, said("Done"))));
    return activitySummary(activity);
  };
  assert.deepEqual(summary(tool("a", "A"), tool("b", "B"), finished("b", "failed")), {
    text: "Worked for 4s, 2 steps",
    failed: 1,
  });
  assert.equal(summary(thought("Hmm")).text, "Thought for 2s");
});

test("times read in the largest units that matter", () => {
  assert.equal(formatWorked(0), "1s");
  assert.equal(formatWorked(12_400), "12s");
  assert.equal(formatWorked(80_000), "1m 20s");
  assert.equal(formatWorked(3_720_000), "1h 2m");
});

test("the live line shows the newest running step", () => {
  const live = (...kinds: SessionEventKind[]) =>
    activities(foldTranscript(events(...kinds)))[0] ?? null;
  const working = session("working");
  assert.equal(liveStepLabel(working, null), "Working");
  assert.equal(liveStepLabel(working, live(thought("Hmm"))), "Thinking");
  assert.equal(liveStepLabel(working, live(tool("a", "Read"), tool("b", "Grep"))), "Grep");
  // The newest finished first, and the older one is what's still going.
  assert.equal(
    liveStepLabel(working, live(tool("a", "Read"), tool("b", "Grep"), finished("b"))),
    "Read",
  );
  // A finished tool stays up until the next one replaces it.
  assert.equal(liveStepLabel(working, live(tool("a", "Read"), finished("a"))), "Read");
});

test("the live line says when it isn't the agent's turn", () => {
  const activity = activities(foldTranscript(events(tool("a", "Read"))))[0];
  assert.equal(liveStepLabel(session("waiting_for_you"), activity), "Waiting for you");
  assert.equal(liveStepLabel(session("starting"), null), "Starting");
});

test("history replaying across several updates never counts as streaming", () => {
  const OPENED = 50_000;
  const history = events(prompt("hi"), said("Hello"), said(" there"), { event: "turn_ended", stop_reason: "end_turn" });
  // However much of it has landed so far.
  for (let landed = 0; landed <= history.length; landed++) {
    const { streaming, isFresh } = transcriptTiming(history.slice(0, landed), OPENED);
    assert.equal(streaming, false, `${landed} events`);
    for (let seq = 1; seq <= landed; seq++) assert.equal(isFresh(seq), false);
  }
});

test("the first event after opening starts streaming, and only new items are fresh", () => {
  const OPENED = 50_000;
  const all = events(
    [1_000, prompt("hi")],
    [2_000, said("Old")],
    [60_000, said(" and growing")],
    [61_000, tool("a", "Read")],
    [62_000, said("New")],
  );
  assert.equal(transcriptTiming(all.slice(0, 2), OPENED).streaming, false);
  const { streaming, isFresh } = transcriptTiming(all, OPENED);
  assert.equal(streaming, true);
  // The message that was already going stays as it was, and only its growth is new.
  assert.equal(isFresh(2), false);
  assert.equal(isFresh(5), true);
});

test("watching a turn unfolds its activities, including ones that start later", () => {
  const before = foldTranscript(events(prompt("go"), tool("a", "Read a"), said("next")));
  const watched = foldState(before, NO_FOLDS, false).toggleTurn();
  const after = foldTranscript(
    events(prompt("go"), tool("a", "Read a"), said("next"), tool("b", "Read b")),
  );
  const fold = foldState(after, watched, false);
  assert.equal(fold.turnOpen, true);
  assert.deepEqual(
    activities(after).map((a) => fold.isOpen(a.key)),
    [true, true],
  );
});

test("an activity pressed by hand wins over its watched turn", () => {
  const items = foldTranscript(events(prompt("go"), tool("a", "Read a")));
  const watched = foldState(items, NO_FOLDS, false).toggleTurn();
  const [only] = activities(items);
  const folded = foldState(items, watched, false).flip(only.key);
  assert.equal(foldState(items, folded, false).isOpen(only.key), false);
  // Pressing the turn again makes the whole turn agree.
  const again = foldState(items, folded, false).toggleTurn();
  assert.equal(foldState(items, again, false).isOpen(only.key), false);
  const open = foldState(items, again, false).toggleTurn();
  assert.equal(foldState(items, open, false).isOpen(only.key), true);
});

test("a watched turn stays unfolded after the next prompt, while the new one follows the setting", () => {
  const first = foldTranscript(events(prompt("one"), tool("a", "Read a")));
  const watched = foldState(first, NO_FOLDS, false).toggleTurn();
  const both = foldTranscript(
    events(prompt("one"), tool("a", "Read a"), prompt("two"), tool("b", "Read b")),
  );
  const fold = foldState(both, watched, false);
  assert.deepEqual(
    activities(both).map((a) => fold.isOpen(a.key)),
    [true, false],
  );
  assert.equal(fold.turnOpen, false);
  // Watching the new turn keeps the old one as it was.
  const next = foldState(both, foldState(both, watched, false).toggleTurn(), false);
  assert.deepEqual(
    activities(both).map((a) => next.isOpen(a.key)),
    [true, true],
  );
});

test("with steps shown, the live line folds the turn", () => {
  const items = foldTranscript(events(prompt("go"), tool("a", "Read a")));
  const fold = foldState(items, NO_FOLDS, true);
  assert.equal(fold.turnOpen, true);
  const folded = foldState(items, fold.toggleTurn(), true);
  assert.equal(folded.isOpen(activities(items)[0].key), false);
});
