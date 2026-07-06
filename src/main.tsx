import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
// Bundled fonts (offline — loading from a CDN would be network egress, which
// Session Signals' privacy guardrail forbids). Geist for UI, Geist Mono for code/labels.
import "@fontsource/geist-sans/latin-400.css";
import "@fontsource/geist-sans/latin-500.css";
import "@fontsource/geist-sans/latin-600.css";
import "@fontsource/geist-sans/latin-700.css";
import "@fontsource/geist-mono/latin-400.css";
import "@fontsource/geist-mono/latin-500.css";
import "./themes/tokens.css";
import { applyThemeToDom, DEFAULT_THEME_ID, getTheme } from "./themes";

// Set the default theme's dot-shape/glow attributes before the first paint so
// dots render correctly from frame one. `useTheme` then reconciles with the
// persisted choice (and pushes the palette to the tray) once mounted.
applyThemeToDom(getTheme(DEFAULT_THEME_ID));

// One Vite bundle serves both Tauri windows. We branch on the window label so
// the widget stays a separate, lighter view (and never pulls in the settings
// styles, keeping its background transparent).
function resolveLabel(): string {
  try {
    // Synchronous in Tauri 2 — available the moment the script runs. The catch
    // covers running outside Tauri (plain `vite dev` in a browser).
    return getCurrentWindow().label;
  } catch {
    return "settings";
  }
}

const root = ReactDOM.createRoot(document.getElementById("root") as HTMLElement);

if (resolveLabel() === "widget") {
  void import("./widget/Widget").then(({ default: Widget }) => {
    root.render(
      <React.StrictMode>
        <Widget />
      </React.StrictMode>,
    );
  });
} else {
  void import("./settings/Settings").then(({ default: Settings }) => {
    root.render(
      <React.StrictMode>
        <Settings />
      </React.StrictMode>,
    );
  });
}
