// Mirrors the Rust `Config` (src-tauri/src/config.rs). Field names are
// snake_case to match the serde shape the `set_config`/`get_config` commands
// deserialize directly.

export interface StateNotify {
  enabled: boolean;
  sound: boolean;
  sound_name: string;
}

/// One session-ignore matcher. Mirrors the serde-tagged Rust `ignore::Matcher`
/// (src-tauri/src/ignore.rs): the `kind` discriminant plus that kind's fields.
/// Hides non-interactive / machine-spawned sessions (e.g. ECC headless
/// `claude --print` agents) from the widget and tray rollup.
export type IgnoreMatcher =
  { kind: "cwd_contains"; value: string } | { kind: "first_prompt_prefix"; value: string };

/// A user-configured addition to the built-in marker registry. Mirrors the
/// Rust `markers::MarkerRule` (src-tauri/src/markers.rs). Additive only — an
/// entry colliding with a built-in prefix is dropped by the backend.
export interface MarkerRule {
  prefix: string;
  polarity: "human" | "machine";
}

export interface Config {
  version: number;
  port: number;
  stale_timeout_min: number;
  /// Minutes of total silence before an idle session is removed from the list.
  /// Until then it stays visible, greyed. Always >= stale_timeout_min.
  idle_drop_min: number;
  launch_on_login: boolean;
  notify_idle: boolean;
  /// Suppress a transition notification when that session's terminal is already
  /// frontmost. On by default; unresolvable terminals always notify.
  notify_unfocused_only: boolean;
  /// Active theme id (see src/themes). Unknown ids fall back to the default.
  theme: string;
  needs_you: StateNotify;
  working: StateNotify;
  ready: StateNotify;
  /// Rules that hide non-interactive / machine-spawned sessions from the widget
  /// and tray rollup. There's no editor UI yet, so this is a typed passthrough:
  /// the settings window loads it via `get_config` and carries it verbatim
  /// through every `set_config` save, so a save never silently drops the user's
  /// rules. `[]` disables filtering. See `ignore::Matcher` in the Rust backend.
  ignore_rules: IgnoreMatcher[];
  /// Openings the user has declared their own — outranks `ignore_rules` and
  /// is never observed. Same passthrough-only shape as `ignore_rules`; `[]`
  /// means nothing is allowlisted.
  never_hide: IgnoreMatcher[];
  /// User-configured additions to the built-in marker registry. Additive
  /// only. `[]` by default.
  markers: MarkerRule[];
  /// Whether Session Signals reads session openings to look for repeating
  /// patterns (salted-hash counts only, never plaintext). On by default.
  observe_enabled: boolean;
  /// Days an observation record is kept before being pruned.
  observe_retain_days: number;
}

/// Built-in notification sounds offered in the UI (macOS system sound names).
export const SOUNDS = ["Ping", "Glass", "Submarine", "Funk", "Pop", "Hero"];

export const DEFAULT_CONFIG: Config = {
  version: 1,
  port: 4317,
  stale_timeout_min: 10,
  idle_drop_min: 60,
  launch_on_login: false,
  notify_idle: false,
  notify_unfocused_only: true,
  theme: "classic",
  needs_you: { enabled: true, sound: false, sound_name: "Ping" },
  working: { enabled: false, sound: false, sound_name: "Pop" },
  ready: { enabled: false, sound: false, sound_name: "Glass" },
  // Mirrors Rust `ignore::IgnoreRules::defaults()` — empty. Session Signals
  // hides nothing until the user opts in. Keeping this empty also means a save
  // landing before the initial `get_config` resolves can never resurrect rules
  // a user deliberately cleared.
  ignore_rules: [],
  never_hide: [],
  markers: [],
  observe_enabled: true,
  observe_retain_days: 30,
};
