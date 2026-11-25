import assert from "node:assert/strict";
import { test } from "node:test";
import type { QuestionField } from "./engine";
import { answerValues, pagesOf } from "./questions.ts";

const choice = (key: string, multiple = false): QuestionField => ({
  key,
  kind: { type: "choice", options: [{ value: "a", label: "A" }], multiple },
});
const text = (key: string, required = false): QuestionField => ({ key, kind: { type: "text" }, required });

test("empty answers are left out and numbers are numbers", () => {
  const fields: QuestionField[] = [
    choice("pick"),
    choice("many", true),
    text("other"),
    { key: "n", kind: { type: "number", integer: true } },
    { key: "ok", kind: { type: "boolean" } },
  ];
  assert.deepEqual(
    answerValues(fields, { pick: "a", many: [], other: "  ", n: "42", ok: false, stray: "x" }),
    { pick: "a", n: 42, ok: false },
  );
});

test("each question gets a page, with its own text boxes", () => {
  const pages = pagesOf([text("intro"), choice("q0"), text("q0_custom"), choice("q1", true), text("q1_custom")]);
  assert.deepEqual(
    pages.map((page) => [page.main?.key ?? null, page.extra.map((f) => f.key)]),
    [
      [null, ["intro"]],
      ["q0", ["q0_custom"]],
      ["q1", ["q1_custom"]],
    ],
  );
  assert.deepEqual(pagesOf([text("a"), text("b")]).length, 1);
});
