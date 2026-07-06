// Shared types mirroring the Rust engine's serialized shapes. The UI never
// derives state from these — it only renders what the engine sends.

export type SessionState = "needs_you" | "working" | "ready";

export type Rollup = "red" | "orange" | "green" | "grey";

export interface SessionView {
  session_id: string;
  /// Combined one-line label ("folder (branch)" or "folder") — for plain-text
  /// surfaces; the widget's two-tone row uses the structured parts below.
  label: string;
  /// The label's structured parts, shipped by the engine so the UI never
  /// re-parses `label` (a folder literally named "foo (bar)" would misparse).
  folder: string;
  branch: string | null;
  /// True when the session's cwd is a linked git worktree. The UI shows a subtle
  /// marker so it's distinguishable from a checkout of the same repo.
  worktree: boolean;
  state: SessionState;
  stale: boolean;
  seconds_in_state: number;
  /// Live subagents running under this session (SubagentStart − SubagentStop).
  subagent_count: number;
  /// Seconds since the subagent count rose from 0 (0 when none are running).
  subagent_seconds: number;
  /// Whether Session Signals resolved the owning terminal window — gates the row's
  /// click-to-focus affordance.
  can_focus: boolean;
  /// Short descriptor of what the session is about — Claude Code's own session
  /// title (else the first prompt), derived locally from the transcript. `null`
  /// until one is available (e.g. a brand-new session). Display-only.
  descriptor: string | null;
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
