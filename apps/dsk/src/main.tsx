import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "@fontsource-variable/inter";
import "@fontsource-variable/geist-mono";
import "../../../design/tokens/generated/tokens.css";
import "./ui/base.css";
import "./ui/app.css";
import { App } from "./App";
import { restoreAppearance } from "./appearance";
import { startSettings } from "./keybinds";

const root = document.getElementById("root");
if (!root) throw new Error("index.html has no #root element");

// Before anything is drawn, so a theme doesn't flash the default one on the way in.
restoreAppearance();
void startSettings();

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
