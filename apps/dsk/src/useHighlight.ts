import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { spanLines } from "./diff";
import type { SyntaxSpan } from "./markdown";

/** Highlights one side of a file off the UI thread, a line of spans per line of text. Each
 * new revision brings new arrays, so it goes by the text, and a file that didn't change keeps
 * its colors. */
export function useHighlight(lines: string[] | null, language: string): SyntaxSpan[][] | null {
  const code = lines && lines.length > 0 ? lines.join("\n") : null;
  const [spans, setSpans] = useState<{ code: string; spans: SyntaxSpan[][] } | null>(null);
  useEffect(() => {
    if (code === null || spans?.code === code) return;
    let active = true;
    invoke<SyntaxSpan[]>("highlight_code", { code, language })
      .then((result) => {
        if (active) setSpans({ code, spans: spanLines(result) });
      })
      .catch(() => {});
    return () => {
      active = false;
    };
  }, [code, language]);
  return spans && spans.code === code ? spans.spans : null;
}
