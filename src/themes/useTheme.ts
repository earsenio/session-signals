import { useEffect } from "react";
import { useConfig } from "../state/useConfig";
import { applyTheme, getTheme } from "./index";
import type { Theme } from "./types";

/// Subscribe to the active theme. Rides `useConfig` (persisted snapshot on
/// mount, live `config-updated` after): the theme is derived directly from
/// `config.theme` (getTheme returns a stable registry object per id), and an
/// effect applies its side effects (DOM attributes + tray palette) whenever it
/// changes — so changing the theme in Settings instantly restyles every window,
/// not just the one that made the change. Returns the resolved theme so
/// components can read its palette colors directly (the dots use these inline).
export function useTheme(): Theme {
  const { theme: themeId } = useConfig();
  const theme = getTheme(themeId);

  useEffect(() => {
    applyTheme(theme);
  }, [theme]);

  return theme;
}
