// Mirrors the Rust `Config` (src-tauri/src/config.rs). Field names are
// snake_case to match the serde shape the `set_config`/`get_config` commands
// deserialize directly.

export interface StateNotify {
  enabled: boolean;
  sound: boolean;
  sound_name: string;
}

export interface Config {
  version: number;
  port: number;
  stale_timeout_min: number;
  launch_on_login: boolean;
  notify_idle: boolean;
  needs_you: StateNotify;
  working: StateNotify;
  ready: StateNotify;
}

/// Built-in notification sounds offered in the UI (macOS system sound names).
export const SOUNDS = ["Ping", "Glass", "Submarine", "Funk", "Pop", "Hero"];

export const DEFAULT_CONFIG: Config = {
  version: 1,
  port: 4317,
  stale_timeout_min: 10,
  launch_on_login: false,
  notify_idle: false,
  needs_you: { enabled: true, sound: false, sound_name: "Ping" },
  working: { enabled: false, sound: false, sound_name: "Pop" },
  ready: { enabled: false, sound: false, sound_name: "Glass" },
};
