// A theme is **pure data**: a palette mapping every state/rollup to a color,
// plus how the status "dot" is drawn (filled disc vs. ring, with/without glow).
// Nothing here is a component or an image asset — the same data drives the React
// dots (via CSS) *and* the native tray icon (rendered in Rust from the pushed
// palette). Adding a theme is therefore one new data file; see ./README.md.

import type { Rollup, SessionState } from "../state/types";

/// The color map. `states` covers the per-session traffic-light states; `rollups`
/// covers the tray rollup (which adds `grey` for "no live sessions"); `stale` is
/// the muted color used for sessions that have gone silent. The *shape* of each
/// state's glyph is fixed (see StateGlyph) — a theme only changes the colors.
export interface ThemePalette {
  states: Record<SessionState, string>;
  rollups: Record<Rollup, string>;
  stale: string;
}

export interface Theme {
  /// Stable identifier persisted in config (never shown to users).
  id: string;
  /// Human name shown in the theme picker.
  name: string;
  /// One-line description shown under the name.
  description: string;
  palette: ThemePalette;
}
