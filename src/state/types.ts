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

export const STATE_LABEL: Record<SessionState, string> = {
  needs_you: "Needs you",
  working: "Working",
  ready: "Ready",
};

export const STATE_COLOR: Record<SessionState, string> = {
  needs_you: "#e53e3e",
  working: "#f59e0b",
  ready: "#22c55e",
};

export const ROLLUP_COLOR: Record<Rollup, string> = {
  red: "#e53e3e",
  orange: "#f59e0b",
  green: "#22c55e",
  grey: "#9ca3af",
};

export const ROLLUP_LABEL: Record<Rollup, string> = {
  red: "A session needs you",
  orange: "Working",
  green: "Ready",
  grey: "No live sessions",
};
