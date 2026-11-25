// `@path` in the message box, as in Claude Code: typing `@` offers the folder's files, and the
// daemon sends the contents of each file a prompt mentions.

export interface Mention {
  /** Where its `@` is. */
  start: number;
  /** What's typed after the `@`, up to the caret. */
  query: string;
}

/** The mention the caret is in, if any: an `@` that starts a word, with no space after it. */
export function mentionAt(text: string, caret: number): Mention | null {
  const before = text.slice(0, caret);
  const match = /(^|[\s(])@([^\s@]*)$/.exec(before);
  if (!match) return null;
  const query = match[2];
  return { start: caret - query.length - 1, query };
}

/** The text with the mention completed to `path`, and where the caret goes after it. */
export function completeMention(
  text: string,
  caret: number,
  mention: Mention,
  path: string,
): { text: string; caret: number } {
  const rest = text.slice(caret).replace(/^\S*/, "");
  const spaced = rest.startsWith(" ") ? rest : ` ${rest}`;
  const inserted = `@${path}`;
  return {
    text: text.slice(0, mention.start) + inserted + spaced,
    caret: mention.start + inserted.length + 1,
  };
}

/** A `!` at the start runs the rest as a shell command, as in Claude Code. */
export function shellLine(text: string): string | null {
  const trimmed = text.trimStart();
  return trimmed.startsWith("!") ? trimmed.slice(1).trim() : null;
}
