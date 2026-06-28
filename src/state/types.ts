// Shared types mirroring the Rust engine's serialized shapes. The UI never
// derives state from these — it only renders what the engine sends.

export type SessionState = "needs_you" | "working" | "ready";

export type Rollup = "red" | "orange" | "green" | "grey";

export interface SessionView {
  session_id: string;
  label: string;
  state: SessionState;
  stale: boolean;
  seconds_in_state: number;
}

export interface SessionsPayload {
  rollup: Rollup;
  sessions: SessionView[];
}

/// Mirrors the Rust `WidgetPrefs` (persisted view preferences).
export interface WidgetPrefs {
  compact: boolean;
  opacity: number;
  visible: boolean;
}

// Appearance (colors, dot style) is NOT defined here — it lives in src/themes
// so it can be swapped at runtime. These maps are text only.

export const STATE_LABEL: Record<SessionState, string> = {
  needs_you: "Needs you",
  working: "Working",
  ready: "Ready",
};

export const ROLLUP_LABEL: Record<Rollup, string> = {
  red: "A session needs you",
  orange: "Working",
  green: "Ready",
  grey: "No live sessions",
};
