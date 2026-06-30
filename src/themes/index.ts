// Theme registry + the glue that applies a theme to the DOM and pushes its
// palette to the native tray. To add a theme: create a data file (copy
// classic/dusk), then add it to THEME_LIST below. Nothing else — no component
// edits, no image assets, no shape changes (shapes are per-state, not per-theme).
// See ./README.md.

import { invoke } from "@tauri-apps/api/core";
import { classic } from "./classic";
import { dusk } from "./dusk";
import type { Theme } from "./types";

export type { Theme, ThemePalette } from "./types";

/// The single place themes are registered. Order = order in the picker.
export const THEME_LIST: Theme[] = [classic, dusk];

export const DEFAULT_THEME_ID = classic.id;

const BY_ID: Record<string, Theme> = Object.fromEntries(THEME_LIST.map((t) => [t.id, t]));

/// Look up a theme by id, falling back to the default for unknown/old ids.
export function getTheme(id: string | undefined): Theme {
  return (id && BY_ID[id]) || classic;
}

/// Write the theme's state colors to CSS custom properties on <html> so the
/// token layer (and any CSS keyed on `--state-*`) restyles live. Colors are the
/// only thing a theme changes; shapes are fixed in StateGlyph.
export function applyThemeToDom(theme: Theme): void {
  const root = document.documentElement;
  const s = theme.palette.states;
  root.style.setProperty("--state-needs", s.needs_you);
  root.style.setProperty("--state-working", s.working);
  root.style.setProperty("--state-ready", s.ready);
  root.style.setProperty("--state-none", theme.palette.stale);
}

/// Convert "#rrggbb" → [r, g, b]. Tolerates a leading '#' and 3-digit shorthand.
export function hexToRgb(hex: string): [number, number, number] {
  let h = hex.replace(/^#/, "");
  if (h.length === 3) {
    h = h[0] + h[0] + h[1] + h[1] + h[2] + h[2];
  }
  const n = parseInt(h, 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

/// Push the theme's palette to the Rust side so the tray glyph (and the themed
/// notification icons) match. The backend draws each shape from these RGB
/// triples, so the tray restyles with no asset and no Rust change. Best-effort:
/// failures (e.g. backend not ready) are swallowed — the backend persists the
/// last palette.
export function pushTrayPalette(theme: Theme): void {
  const p = theme.palette;
  const palette = {
    needs_you: hexToRgb(p.states.needs_you),
    working: hexToRgb(p.states.working),
    ready: hexToRgb(p.states.ready),
    red: hexToRgb(p.rollups.red),
    orange: hexToRgb(p.rollups.orange),
    green: hexToRgb(p.rollups.green),
    grey: hexToRgb(p.rollups.grey),
  };
  invoke("set_tray_palette", { palette }).catch(() => {});
}

/// Apply a theme everywhere it has a side effect: CSS vars + tray palette.
export function applyTheme(theme: Theme): void {
  applyThemeToDom(theme);
  pushTrayPalette(theme);
}
