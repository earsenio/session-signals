import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Config } from "../state/config";
import { applyTheme, DEFAULT_THEME_ID, getTheme } from "./index";
import type { Theme } from "./types";

/// Subscribe to the active theme. Reads `config.theme` on mount, applies it
/// (DOM attributes + tray palette), and re-applies whenever the backend emits
/// `config-updated` — so changing the theme in Settings instantly restyles every
/// window, not just the one that made the change. Returns the resolved theme so
/// components can read its palette colors directly (the dots use these inline).
export function useTheme(): Theme {
  const [theme, setTheme] = useState<Theme>(() => getTheme(DEFAULT_THEME_ID));

  useEffect(() => {
    let active = true;
    const apply = (id: string | undefined) => {
      const t = getTheme(id);
      applyTheme(t);
      if (active) setTheme(t);
    };

    invoke<Config>("get_config")
      .then((c) => apply(c.theme))
      .catch(() => apply(DEFAULT_THEME_ID));

    const un = listen<Config>("config-updated", (e) => apply(e.payload.theme));
    return () => {
      active = false;
      un.then((u) => u());
    };
  }, []);

  return theme;
}
