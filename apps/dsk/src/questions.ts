// What a question's form sends back, apart from how it looks.

import type { QuestionField } from "./engine";

export type Value = string | string[] | boolean;

/** The values as the daemon takes them: numbers as numbers, and nothing for what's empty. */
export function answerValues(
  fields: QuestionField[],
  values: Record<string, Value>,
): Record<string, string | number | boolean | string[]> {
  const out: Record<string, string | number | boolean | string[]> = {};
  for (const field of fields) {
    const value = values[field.key];
    if (value === undefined) continue;
    if (Array.isArray(value)) {
      if (value.length > 0) out[field.key] = value;
    } else if (typeof value === "boolean") {
      out[field.key] = value;
    } else if (value.trim() !== "") {
      const number = Number(value);
      out[field.key] = field.kind.type === "number" && Number.isFinite(number) ? number : value;
    }
  }
  return out;
}

/** One question of a form, asked on its own: a choice, number or flag, with the text boxes
 * that follow it, like Claude's "Other" box beside each of its questions. A form that starts with
 * text boxes asks those first. */
export interface Page {
  main: QuestionField | null;
  extra: QuestionField[];
}

export function pagesOf(fields: QuestionField[]): Page[] {
  const pages: Page[] = [];
  for (const field of fields) {
    const text = field.kind.type === "text" || field.kind.type === "unknown";
    const last = pages[pages.length - 1];
    if (text && last) {
      last.extra.push(field);
    } else if (text) {
      pages.push({ main: null, extra: [field] });
    } else {
      pages.push({ main: field, extra: [] });
    }
  }
  return pages;
}
