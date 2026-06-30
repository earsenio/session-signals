// State/rollup → glyph shape mapping. Kept separate from StateGlyph.tsx so that
// component file only exports a component (React Fast Refresh requirement), and
// so these pure functions can be imported without pulling in the SVG component.

import type { Rollup, SessionState } from "../state/types";

export type GlyphShape = "square" | "dot" | "check" | "ring";

/// needs you → square · working → dot · ready → check.
export function shapeForState(state: SessionState): GlyphShape {
  return state === "needs_you" ? "square" : state === "working" ? "dot" : "check";
}

/// Tray rollup → shape (grey/none → ring).
export function shapeForRollup(rollup: Rollup): GlyphShape {
  return rollup === "red"
    ? "square"
    : rollup === "orange"
      ? "dot"
      : rollup === "green"
        ? "check"
        : "ring";
}
