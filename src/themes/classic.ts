import type { Theme } from "./types";

/// The default traffic-light map. Shapes (square/dot/check/ring) are shared with
/// every theme — only these colors change. Hexes are 1:1 with the design sheet.
export const classic: Theme = {
  id: "classic",
  name: "Classic",
  description: "Traffic-light colors — red, amber, green.",
  palette: {
    states: {
      needs_you: "#f4595e",
      working: "#f5a742",
      ready: "#46c98b",
    },
    rollups: {
      red: "#f4595e",
      orange: "#f5a742",
      green: "#46c98b",
      grey: "#7c828d",
    },
    stale: "#7c828d",
  },
};
