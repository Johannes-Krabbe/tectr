// An agent's message as formatted text. This file avoids JSX so `mise run dsk:test` can
// load it without a build step.

import { invoke } from "@tauri-apps/api/core";
import {
  createElement,
  memo,
  type MouseEvent,
  type ReactNode,
  useEffect,
  useRef,
  useState,
} from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";

const HEADING_LEVELS = [1, 2, 3, 4, 5, 6] as const;

/** Headings keep the text size, so their marker is what shows the level. */
const heading =
  (level: (typeof HEADING_LEVELS)[number]) =>
  ({ children }: { children?: ReactNode }) =>
    createElement(
      `h${level}`,
      null,
      createElement("span", { className: "md-mark" }, `${"#".repeat(level)} `),
      children,
    );

/** Opens a web address in the browser. Leaving out `href` means a click can never send the
 * app's only window there instead, and anything else, like a file path, is just text. */
const reference = (target: string | undefined, children: ReactNode) => {
  if (!target || !/^https?:\/\//i.test(target)) {
    return createElement("span", { title: target }, children);
  }
  return createElement(
    "a",
    {
      className: "md-link",
      title: target,
      onClick: () => {
        // Selecting the link's text isn't asking to open it.
        if (!window.getSelection()?.isCollapsed) return;
        invoke("open_url", { url: target }).catch(console.error);
      },
    },
    children,
  );
};

/** What a piece of code is. These match `Role` in crates/highlight. */
export type SyntaxRole =
  | "plain"
  | "keyword"
  | "string"
  | "constant"
  | "function"
  | "type"
  | "comment"
  | "punctuation"
  | "added"
  | "deleted";

export interface SyntaxSpan {
  role: SyntaxRole;
  text: string;
}

/** Colored spans cover what's been highlighted so far. While a block streams, the text that
 * arrived since stays plain until they catch up. */
export function spansFor(code: string, colored: SyntaxSpan[]): SyntaxSpan[] {
  const covered = colored.map((span) => span.text).join("");
  if (!code.startsWith(covered)) return [{ role: "plain", text: code }];
  const rest = code.slice(covered.length);
  return rest ? [...colored, { role: "plain", text: rest }] : colored;
}

/** Highlights off the UI thread. A block can change every frame while it streams, so only
 * one request runs at a time, and the next one asks for whatever is newest by then. */
function useHighlight(code: string, language: string): SyntaxSpan[] {
  const [colored, setColored] = useState<SyntaxSpan[]>([]);
  const latest = useRef({ code, language });
  const running = useRef(false);
  latest.current = { code, language };

  useEffect(() => {
    if (running.current) return;
    const run = () => {
      const asked = latest.current;
      running.current = true;
      invoke<SyntaxSpan[]>("highlight_code", asked)
        .catch(() => [])
        .then((spans) => {
          running.current = false;
          setColored(spans);
          const { code, language } = latest.current;
          if (code !== asked.code || language !== asked.language) run();
        });
    };
    run();
  }, [code, language]);

  return spansFor(code, colored);
}

function HighlightedCode({ code, language }: { code: string; language: string }) {
  const spans = useHighlight(code, language);
  return createElement(
    "code",
    null,
    spans.map((span, index) =>
      createElement("span", { key: index, className: `syntax-${span.role}` }, span.text),
    ),
  );
}

/** For a block that scrolls sideways, which otherwise keeps the chat from scrolling while
 * the pointer is over it. Sideways scrolling stays native. The listener sits on the block
 * and not the transcript, because one that can cancel scrolling slows down all of it. */
function useChatScrollsThrough<T extends HTMLElement>() {
  const block = useRef<T>(null);
  useEffect(() => {
    const el = block.current;
    if (!el) return;
    const onWheel = (event: WheelEvent) => {
      const chat = el.closest(".log");
      if (!chat || Math.abs(event.deltaY) <= Math.abs(event.deltaX)) return;
      event.preventDefault();
      chat.scrollTop += event.deltaY;
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);
  return block;
}

const COPY_LABEL = "Copy";

function CodeBlock({ children }: { children?: ReactNode }) {
  const pre = useChatScrollsThrough<HTMLPreElement>();
  const [label, setLabel] = useState(COPY_LABEL);

  useEffect(() => {
    if (label === COPY_LABEL) return;
    const timer = setTimeout(() => setLabel(COPY_LABEL), 1500);
    return () => clearTimeout(timer);
  }, [label]);

  const copy = () => {
    // The compiler ends every block with a newline that isn't part of the code.
    const code = pre.current?.textContent?.replace(/\n$/, "") ?? "";
    navigator.clipboard.writeText(code).then(
      () => setLabel("Copied"),
      () => setLabel("Couldn't copy"),
    );
  };

  // Outside the <pre>, so it stays put when the code scrolls sideways.
  return createElement(
    "div",
    { className: "md-code" },
    createElement("pre", { ref: pre }, children),
    createElement(
      "button",
      {
        type: "button",
        className: "md-copy",
        "aria-label": "Copy code",
        // Copying leaves the keyboard wherever it was, such as in the composer.
        onMouseDown: (event: MouseEvent<HTMLButtonElement>) => event.preventDefault(),
        onClick: copy,
      },
      label,
    ),
  );
}

function Table({ children }: { children?: ReactNode }) {
  return createElement("table", { ref: useChatScrollsThrough<HTMLTableElement>() }, children);
}

const COMPONENTS: Components = {
  ...Object.fromEntries(HEADING_LEVELS.map((level) => [`h${level}`, heading(level)])),
  pre: ({ children }) => createElement(CodeBlock, null, children),
  table: ({ children }) => createElement(Table, null, children),
  a: ({ href, children }) => reference(href, children),
  // Agents shouldn't be able to make the app fetch a URL just by mentioning it.
  img: ({ src, alt }) => reference(src, alt || src),
  // Only a fence that names its language has this class, so inline code stays as it is.
  code: ({ className, children }) => {
    const language = className?.match(/language-(\S+)/)?.[1];
    return language
      ? createElement(HighlightedCode, { code: String(children), language })
      : createElement("code", { className }, children);
  },
};

const PLUGINS = [remarkGfm];

/** Memoized because the transcript refolds on every event, while only the newest message
 * has changed. */
export const Markdown = memo(function Markdown({ text }: { text: string }) {
  return createElement(ReactMarkdown, { remarkPlugins: PLUGINS, components: COMPONENTS }, text);
});
