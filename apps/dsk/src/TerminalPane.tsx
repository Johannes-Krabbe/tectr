import { Channel, invoke } from "@tauri-apps/api/core";
import { FitAddon } from "@xterm/addon-fit";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";
import { type ITheme, Terminal as XTerm } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { useEffect, useRef } from "react";
import { shownValue } from "./appearance";
import type { Terminal, TerminalColors } from "./engine";
import { useSettings } from "./keybinds";
import { hint, useHints } from "./shortcuts";
import { binaryBytes, opaque, OUTPUT, readRecords, readReset, RESET } from "./terminalFrames";

const ANSI = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];

const cssValue = (name: string) =>
  getComputedStyle(document.documentElement).getPropertyValue(name).trim();

/** The theme's colors, read from what the appearance settings put on the page. */
function themeColors(): ITheme {
  const color = (token: string) => cssValue(`--color-${token}`);
  const theme: ITheme = {
    background: color("surface-panel"),
    foreground: color("text-primary"),
    cursor: color("text-primary"),
    cursorAccent: color("surface-panel"),
    selectionBackground: color("accent-tint"),
    selectionInactiveBackground: color("accent-tint"),
    scrollbarSliderBackground: color("line-subtle"),
    scrollbarSliderHoverBackground: color("line-strong"),
    scrollbarSliderActiveBackground: color("line-strong"),
  };
  const palette: Record<string, string> = {};
  for (const name of ANSI) {
    palette[name] = color(`terminal-${name}`);
    palette[`bright${name[0].toUpperCase()}${name.slice(1)}`] = color(`terminal-bright-${name}`);
  }
  return { ...theme, ...palette };
}

/** What programs asking the terminal for its colors are told. */
function daemonColors(theme: ITheme): TerminalColors {
  const palette = ANSI.map((name) => opaque(cssValue(`--color-terminal-${name}`))).concat(
    ANSI.map((name) => opaque(cssValue(`--color-terminal-bright-${name}`))),
  );
  return {
    foreground: opaque(theme.foreground ?? ""),
    background: opaque(theme.background ?? ""),
    cursor: opaque(theme.cursor ?? ""),
    palette,
  };
}

/** The code font. A canvas can't read CSS variables, so the stack is spelled out. */
function fontFamily(): string {
  const stack = cssValue("--font-code-stack");
  const chosen = String(shownValue("appearance.code.family") ?? "");
  return chosen ? `"${chosen}", ${stack}` : stack;
}

interface Options {
  fontFamily: string;
  fontSize: number;
  lineHeight: number;
  cursorStyle: "block" | "bar" | "underline";
  cursorBlink: boolean;
  macOptionIsMeta: boolean;
  scrollback: number;
}

function options(): Options {
  const cursor = shownValue("terminal.cursor");
  return {
    fontFamily: fontFamily(),
    fontSize: Number(shownValue("terminal.size") ?? 13),
    lineHeight: Number(shownValue("terminal.line_height") ?? 1),
    cursorStyle: cursor === "bar" || cursor === "underline" ? cursor : "block",
    cursorBlink: shownValue("terminal.cursor_blink") === true,
    macOptionIsMeta: shownValue("terminal.option_as_meta") === true,
    scrollback: Number(shownValue("terminal.scrollback") ?? 10000),
  };
}

/** Programs ask the terminal things, like where the cursor is. The daemon's own copy of the
 * terminal answers, even with no client showing it, so these would be second answers. */
function leaveQueriesToTheDaemon(term: XTerm) {
  const handled = () => true;
  const { parser } = term;
  for (const id of [
    { final: "c" },
    { prefix: ">", final: "c" },
    { prefix: "=", final: "c" },
    { final: "n" },
    { prefix: "?", final: "n" },
    { intermediates: "$", final: "p" },
    { prefix: "?", intermediates: "$", final: "p" },
    { prefix: ">", final: "q" },
    { prefix: "?", final: "u" },
  ]) {
    parser.registerCsiHandler(id, handled);
  }
  for (const id of [
    { intermediates: "$", final: "q" },
    { intermediates: "+", final: "q" },
  ]) {
    parser.registerDcsHandler(id, handled);
  }
  // Setting a color still applies here. Only asking for one goes to the daemon.
  for (const osc of [4, 10, 11, 12]) {
    parser.registerOscHandler(osc, (data) => data.includes("?"));
  }
}

/** A shell, drawn by xterm.js from what the daemon's copy of the terminal sends. Every key
 * goes to the shell except Cmd shortcuts, which stay the app's. */
export function TerminalPane({
  machineId,
  terminal,
  online,
}: {
  machineId: string;
  terminal: Terminal;
  online: boolean;
}) {
  const host = useRef<HTMLDivElement>(null);
  const live = useRef<{ term: XTerm; fit: FitAddon; claim: () => void } | null>(null);
  // Rendering again when the settings change is what applies them below.
  useSettings();
  useHints("pane", [hint("Sidebar", "global.toggle_sidebar")]);

  useEffect(() => {
    const element = host.current;
    if (!element || !online) return;
    const terminalId = terminal.id;
    let disposed = false;
    let cleanup = () => {};

    const start = async () => {
      // Cells are measured once, so the font has to be there first.
      const settings = options();
      await document.fonts
        .load(`${settings.fontSize}px ${settings.fontFamily}`)
        .catch(() => undefined);
      if (disposed) return;

      const theme = themeColors();
      const term = new XTerm({ ...settings, theme, allowProposedApi: true });
      const fit = new FitAddon();
      term.loadAddon(fit);
      term.loadAddon(new Unicode11Addon());
      term.unicode.activeVersion = "11";
      term.loadAddon(
        new WebLinksAddon((_event, url) => {
          void invoke("open_url", { url }).catch(() => {});
        }),
      );
      term.open(element);
      try {
        const webgl = new WebglAddon();
        // A webview has a few WebGL contexts to share. Losing this one falls back to the DOM.
        webgl.onContextLoss(() => webgl.dispose());
        term.loadAddon(webgl);
      } catch {
        // Without WebGL, xterm.js draws with the DOM.
      }
      leaveQueriesToTheDaemon(term);
      term.attachCustomKeyEventHandler((event) => !event.metaKey);
      fit.fit();

      // Acknowledging once a frame instead of per write keeps this cheap during a flood.
      let drawn = 0;
      let acking = false;
      const ack = (bytes: number) => {
        drawn += bytes;
        if (acking) return;
        acking = true;
        requestAnimationFrame(() => {
          acking = false;
          const count = drawn;
          drawn = 0;
          void invoke("terminal_ack", { machineId, terminalId, bytes: count }).catch(() => {});
        });
      };

      // The daemon's size, which every client showing it draws at.
      let size = { cols: term.cols, rows: term.rows };
      const frames = new Channel<ArrayBuffer>();
      frames.onmessage = (buffer) => {
        for (const { kind, payload } of readRecords(buffer)) {
          if (kind === OUTPUT) {
            term.write(payload, () => ack(payload.length));
          } else if (kind === RESET) {
            const { cols, rows, screen } = readReset(payload);
            size = { cols, rows };
            term.reset();
            if (cols !== term.cols || rows !== term.rows) term.resize(cols, rows);
            term.write(screen, () => ack(payload.length));
          }
        }
      };
      void invoke("terminal_attach", {
        machineId,
        terminalId,
        cols: term.cols,
        rows: term.rows,
        colors: daemonColors(theme),
        frames,
      }).catch(() => {});

      // The client in use sets the size, so one that's typed into or resized takes it back.
      const claim = () => {
        const fitted = fit.proposeDimensions();
        if (!fitted || !fitted.cols || !fitted.rows) return;
        if (fitted.cols === size.cols && fitted.rows === size.rows) return;
        size = { cols: fitted.cols, rows: fitted.rows };
        term.resize(fitted.cols, fitted.rows);
        void invoke("terminal_resize", {
          machineId,
          terminalId,
          cols: fitted.cols,
          rows: fitted.rows,
        }).catch(() => {});
      };
      const send = (bytes: Uint8Array) => {
        claim();
        void invoke("terminal_input", bytes, {
          headers: { "machine-id": machineId, "terminal-id": terminalId },
        }).catch(() => {});
      };
      const encoder = new TextEncoder();
      const typed = term.onData((data) => send(encoder.encode(data)));
      const typedBinary = term.onBinary((data) => send(binaryBytes(data)));
      term.textarea?.addEventListener("focus", claim);

      let resizing = 0;
      const observer = new ResizeObserver(() => {
        cancelAnimationFrame(resizing);
        resizing = requestAnimationFrame(() => {
          if (document.hasFocus()) claim();
        });
      });
      observer.observe(element);

      // The appearance settings repaint by setting variables on the page.
      const repaint = new MutationObserver(() => {
        term.options.theme = themeColors();
      });
      repaint.observe(document.documentElement, { attributes: true, attributeFilter: ["style"] });

      live.current = { term, fit, claim };
      // The pane was given the keyboard before xterm.js had somewhere to put it.
      if (document.activeElement === element) term.focus();
      cleanup = () => {
        live.current = null;
        observer.disconnect();
        repaint.disconnect();
        cancelAnimationFrame(resizing);
        typed.dispose();
        typedBinary.dispose();
        void invoke("terminal_detach", { machineId, terminalId }).catch(() => {});
        term.dispose();
      };
    };
    void start();
    return () => {
      disposed = true;
      cleanup();
    };
  }, [machineId, terminal.id, online]);

  // Settings that change while it's open apply in place.
  const settings = useRef(options());
  settings.current = options();
  const settingsKey = JSON.stringify(settings.current);
  useEffect(() => {
    const current = live.current;
    if (!current) return;
    const { term, claim } = current;
    const next = settings.current;
    void document.fonts.load(`${next.fontSize}px ${next.fontFamily}`).finally(() => {
      Object.assign(term.options, next);
      claim();
    });
  }, [settingsKey]);

  return (
    <div className="pane terminal">
      <div
        ref={host}
        className="terminal-host"
        tabIndex={-1}
        onFocus={(event) => {
          if (event.target === host.current) live.current?.term.focus();
        }}
      />
    </div>
  );
}
