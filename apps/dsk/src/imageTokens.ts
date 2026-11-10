// How a message's text refers to its images: `[Image #3]` is the image numbered 3. The text
// is the source of truth, so deleting a token drops its image and undo brings it back.

import type { PromptImage } from "./engine";

const TOKEN = /\[Image #(\d+)\]/g;

export interface TokenSpan {
  number: number;
  start: number;
  end: number;
}

export const tokenOf = (number: number) => `[Image #${number}]`;

export function tokensIn(text: string): TokenSpan[] {
  return [...text.matchAll(TOKEN)].map((match) => ({
    number: Number(match[1]),
    start: match.index,
    end: match.index + match[0].length,
  }));
}

/** The images the text refers to, in the order it first does. A token for an image nobody
 * pasted is only text. */
export function imagesIn(text: string, known: ReadonlyMap<number, PromptImage>): PromptImage[] {
  const found: PromptImage[] = [];
  for (const { number } of tokensIn(text)) {
    const image = known.get(number);
    if (image && !found.includes(image)) found.push(image);
  }
  return found;
}

/** The token the cursor is in or touches on either side. */
export function tokenAt(text: string, cursor: number): TokenSpan | undefined {
  return tokensIn(text).find((token) => token.start <= cursor && cursor <= token.end);
}

/** The token Backspace would cut into, or Delete when `forward`. */
export function tokenBeside(text: string, cursor: number, forward: boolean): TokenSpan | undefined {
  return tokensIn(text).find((token) => (forward ? token.start === cursor : token.end === cursor));
}

export const highestNumber = (images: readonly PromptImage[]) =>
  images.reduce((highest, image) => Math.max(highest, image.number), 0);

/** Numbers count up through a session, so `sent` is the highest one it has sent so far.
 * Numbers a draft has let go of are free again. */
export function nextNumber(sent: number, draft: readonly PromptImage[]): number {
  return Math.max(sent, highestNumber(draft)) + 1;
}

/** The token to type at the cursor, set apart from the words on either side. */
export function insertion(text: string, cursor: number, number: number): string {
  const before = cursor > 0 && !/\s/.test(text[cursor - 1]) ? " " : "";
  const after = cursor < text.length && /\s/.test(text[cursor]) ? "" : " ";
  return `${before}${tokenOf(number)}${after}`;
}

/** Text with each token cut out as its own piece, for drawing tokens apart from the words. */
export function splitTokens(
  text: string,
  known: ReadonlyMap<number, PromptImage>,
): ({ text: string } | { text: string; image: PromptImage })[] {
  const pieces: ({ text: string } | { text: string; image: PromptImage })[] = [];
  let at = 0;
  for (const token of tokensIn(text)) {
    const image = known.get(token.number);
    if (!image) continue;
    if (token.start > at) pieces.push({ text: text.slice(at, token.start) });
    pieces.push({ text: text.slice(token.start, token.end), image });
    at = token.end;
  }
  if (at < text.length) pieces.push({ text: text.slice(at) });
  return pieces;
}

export const byNumber = (images: readonly PromptImage[]) =>
  new Map(images.map((image) => [image.number, image]));
