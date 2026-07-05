# Themes

A theme in Session Signals is **pure data** — a color map, nothing else. The *shape* of
each state's indicator (square / dot / check / ring) is fixed in `StateGlyph`
and shared by every theme; a theme only swaps colors. The same data drives the
React glyphs *and* the native menu-bar/tray icon (Rust renders the shapes from
the palette pushed up at runtime), so a theme needs no image assets.

## What a theme controls

- One color per session state (`needs_you`, `working`, `ready`).
- One per tray rollup (`red`, `orange`, `green`, `grey`).
- A `stale` color for sessions that have gone silent.

## Built-in themes

- **Classic** — traffic-light red / amber / green.
- **Dusk** — softer, desaturated alternate.

## Add a theme (one file + one line)

1. Copy `classic.ts` to `mytheme.ts` and edit `id`, `name`, `description`, and
   `palette`.
2. Register it in `index.ts`:

   ```ts
   import { myTheme } from "./myTheme";
   export const THEME_LIST: Theme[] = [classic, dusk, myTheme];
   ```

It then appears in the Settings theme control and, when selected, instantly
restyles the widget, settings, tray glyph, and notification icons — no component
edits, no Rust changes, no PNGs.

## How it reaches the rest of the app

- `applyThemeToDom()` writes the palette to `--state-*` CSS custom properties on
  `<html>`; the token layer and components read those. The static surface/type/
  spacing tokens live in `tokens.css`.
- `pushTrayPalette()` sends the colors (RGB triples) to Rust, which draws the
  square/dot/check/ring tray + notification glyphs from them.
