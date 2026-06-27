import React from "react";
import ReactDOM from "react-dom/client";

// One Vite bundle serves both Tauri windows. We branch on the window label so
// the widget stays a separate, lighter view (and never pulls in the settings
// styles, keeping its background transparent).
function resolveLabel(): string {
  try {
    // `__TAURI_INTERNALS__` carries the current window's metadata. Reading it
    // avoids an async import and works the moment the script runs.
    const internals = (window as unknown as {
      __TAURI_INTERNALS__?: { metadata?: { currentWindow?: { label?: string } } };
    }).__TAURI_INTERNALS__;
    return internals?.metadata?.currentWindow?.label ?? "settings";
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
