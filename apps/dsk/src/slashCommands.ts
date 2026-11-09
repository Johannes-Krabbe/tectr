// The slash commands an agent offers, as the message box suggests them. Running one is just
// sending its text as a prompt, so nothing here talks to the agent.

import type { AgentCommand } from "./engine";

/** Commands that change what tectr sets itself, and what that is. The daemon leaves them out of
 * the lists and refuses them, and checking here too means a new agent isn't started for a prompt
 * that would be refused. This mirrors `TECTR_OWNED_COMMANDS` in crates/protocol, which a test
 * there checks. */
export const TECTR_OWNED_COMMANDS: Record<string, string> = {
  model: "the model",
  effort: "the effort",
  permissions: "the permission mode",
  approvals: "the permission mode",
  config: "the model and permission mode",
  mcp: "the MCP servers",
};

/** Why a prompt can't be sent, the same as the daemon would say, or null if it can. */
export function refusedPrompt(text: string): string | null {
  const name = /^\s*\/(\S+)/.exec(text)?.[1];
  if (name === undefined || !Object.hasOwn(TECTR_OWNED_COMMANDS, name)) return null;
  return `tectr sets ${TECTR_OWNED_COMMANDS[name]}, so /${name} can't run here. Change that in tectr instead.`;
}

/** The part of a name typed so far, while the message is only `/` and that. Null otherwise,
 * so the list goes away as soon as there's a space, a second line or anything before it. */
export function commandQuery(text: string): string | null {
  const match = /^\/(\S*)$/.exec(text);
  return match ? match[1] : null;
}

/** Names starting with what's typed come first, then names containing it, each in the
 * agent's own order. */
export function matchCommands(commands: AgentCommand[], query: string): AgentCommand[] {
  const typed = query.toLowerCase();
  const starting: AgentCommand[] = [];
  const containing: AgentCommand[] = [];
  for (const command of commands) {
    const name = command.name.toLowerCase();
    if (name.startsWith(typed)) starting.push(command);
    else if (name.includes(typed)) containing.push(command);
  }
  return [...starting, ...containing];
}

/** The text a chosen command leaves in the box, ready for whatever goes after it. */
export const completedText = (command: AgentCommand) => `/${command.name} `;

/** What the send key does with a command chosen. One that takes input is completed so it can
 * be typed, and one that doesn't is sent right away. */
export const sendsAtOnce = (command: AgentCommand) => !command.hint;
