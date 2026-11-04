# Wording

The same words appear in both apps, so the product reads as one thing.

## Terms

| Use | Not | Meaning |
|---|---|---|
| Device | Machine, host, server | A computer running a daemon |
| Workspace | Project, repo | A repo on one device |
| Topic | Task, feature, thread | A feature or task inside a workspace |
| Agent | Bot, assistant | What you see in a tab. "Session" stays in code and docs |
| Changes | Diff | The tab showing what a topic changed |
| Terminal | Shell, console | A terminal tab |
| Request | Prompt, approval | Something an agent needs from you |

`machine_id` and "session" stay as they are in code and in `architecture.md`.

## States

Working, Waiting, Failed, Done, Idle, Offline. Use the same word in every place a state appears.

## Rules

- **Sentence case** everywhere, including buttons and titles.
- **Buttons name the action:** "Send 3 comments", not "Submit" or "OK".
- **Say what will happen or why:** "Deletes files outside version control", not "Warning".
- **Explain refusals in one line:** "VPS is offline, so nothing can be created there".
- **No filler:** no "please", no "successfully", no exclamation marks.
- **Times:** running time as `4:12`, and past events as `6m`, `2h`, or `2h ago` when the context needs it.
