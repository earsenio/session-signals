// The state glyph — four distinct silhouettes so state reads by SHAPE, not hue
// (greyscale-safe, legible at 16px). Shape is a fixed function of the state;
// the theme only supplies the color. Geometry is 1:1 with the design's Glyph.

import type { GlyphShape } from "./glyphShape";

// The breathing ring's geometry, expressed in the SVG's 24-unit viewBox so the
// DOM ring below stays 1:1 with the circle it replaced: r=8.5 with a 2-wide
// stroke, i.e. an outer diameter of (8.5 + 1) * 2.
const RING_VIEWBOX = 24;
const RING_OUTER = 19;
const RING_STROKE = 2;

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
  const svg = (
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
      {shape === "dot" && <circle cx="12" cy="12" r="5.4" fill={color} />}
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

  if (!pulse || shape !== "dot") return svg;

  // The ring is a DOM element, NOT an SVG child, and that is the whole point:
  // WebKit cannot promote a non-root SVG child to its own compositing layer, so
  // animating one — even on transform/opacity — falls back to the main thread
  // and repaints the widget on every frame, forever, at the display's refresh
  // rate. A plain span with `will-change` gets its own layer and the same
  // keyframes run compositor-side for free. Centered with negative margins
  // rather than a translate so `transform` stays a pure scale() and the shared
  // `beaconPulse` keyframes need no offset baked into them.
  const outer = (RING_OUTER / RING_VIEWBOX) * size;
  return (
    <span className="glyphWrap" style={{ width: size, height: size }}>
      <span
        className="glyphPulse"
        style={{
          width: outer,
          height: outer,
          marginTop: -outer / 2,
          marginLeft: -outer / 2,
          borderWidth: (RING_STROKE / RING_VIEWBOX) * size,
          borderColor: color,
        }}
      />
      {svg}
    </span>
  );
}
