import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "@fontsource-variable/inter";
import "@fontsource-variable/geist-mono";
import "../../../design/tokens/generated/tokens.css";
import "./ui/base.css";
import "./ui/app.css";
import { App } from "./App";
import { traceFocus } from "./focusTrace";
import { restoreAppearance } from "./appearance";
import { startSettings } from "./keybinds";
import { keepButtonsInTabOrder, keepKeyboardSomewhere } from "./keys";
import { followScrolling } from "./scrollbars";

const root = document.getElementById("root");
if (!root) throw new Error("index.html has no #root element");

// TEMPORARY: finding where the keyboard goes when it sometimes ends up nowhere.
if (import.meta.env.DEV) traceFocus();
// Before anything is drawn, so a theme doesn't flash the default one on the way in.
restoreAppearance();
void startSettings();
followScrolling();
// The body rather than the root, because menus and sheets are drawn outside it.
keepButtonsInTabOrder(document.body);
keepKeyboardSomewhere();

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
