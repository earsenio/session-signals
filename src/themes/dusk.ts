import type { Theme } from "./types";

/// The alternate: a softer, desaturated map for people who find the traffic
/// light too loud. Same shapes, calmer colors. Hexes are 1:1 with the design.
export const dusk: Theme = {
  id: "dusk",
  name: "Dusk",
  description: "Softer, desaturated — easier on the eyes.",
  palette: {
    states: {
      needs_you: "#cc6f76",
      working: "#c99b5a",
      ready: "#6fb89a",
    },
    rollups: {
      red: "#cc6f76",
      orange: "#c99b5a",
      green: "#6fb89a",
      grey: "#6e737d",
    },
    stale: "#6e737d",
  },
};
