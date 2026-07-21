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
  | { kind: "cwd_contains"; value: string }
  | { kind: "folder_hex"; min_len: number }
  | { kind: "first_prompt_prefix"; value: string };

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
  // Mirrors Rust `ignore::IgnoreRules::defaults()`. Only used before the initial
  // `get_config` resolves; after that the backend's persisted rules replace it.
  ignore_rules: [
    { kind: "cwd_contains", value: "ecc-homunculus" },
    { kind: "folder_hex", min_len: 12 },
    { kind: "first_prompt_prefix", value: "IMPORTANT: You are running in non-interactive" },
  ],
};
