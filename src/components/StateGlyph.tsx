// The state glyph — four distinct silhouettes so state reads by SHAPE, not hue
// (greyscale-safe, legible at 16px). Shape is a fixed function of the state;
// the theme only supplies the color. Geometry is 1:1 with the design's Glyph.

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

export function StateGlyph({
  shape,
  color,
  size = 22,
  pulse = false,
}: {
  shape: GlyphShape;
  color: string;
  size?: number;
  /// Only meaningful for the dot — adds the breathing outer ring.
  pulse?: boolean;
}) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      style={{ display: "block", overflow: "visible", flex: "none" }}
      aria-hidden="true"
    >
      {shape === "square" && (
        <rect x="4.6" y="4.6" width="14.8" height="14.8" rx="3.6" fill={color} />
      )}
      {shape === "dot" && (
        <>
          {pulse && (
            <circle
              cx="12"
              cy="12"
              r="8.5"
              fill="none"
              stroke={color}
              strokeWidth="2"
              style={{
                animation: "beaconPulse 2.6s ease-in-out infinite",
                transformOrigin: "12px 12px",
              }}
            />
          )}
          <circle cx="12" cy="12" r="5.4" fill={color} />
        </>
      )}
      {shape === "check" && (
        <path
          d="M5 12.6 L10 17.4 L19.3 6.8"
          fill="none"
          stroke={color}
          strokeWidth="3.2"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      )}
      {shape === "ring" && (
        <circle cx="12" cy="12" r="7.4" fill="none" stroke={color} strokeWidth="2.6" />
      )}
    </svg>
  );
}
