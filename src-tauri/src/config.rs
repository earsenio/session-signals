//! User configuration: notification preferences + listener/runtime settings.
//!
//! Persisted as a single `config` object inside the shared `beacon.json` store.
//! Every field carries `#[serde(default)]`, so a config written by an older
//! build (missing newer keys) still loads — the missing keys fall back to their
//! defaults. `version` lets us run an explicit migration later if the shape
//! changes in a way defaults can't cover; for now `sanitized()` normalizes it.

use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_plugin_store::StoreExt;

const STORE_FILE: &str = "beacon.json";
const CONFIG_KEY: &str = "config";

/// Bump when the schema changes in a way that needs active migration.
pub const CURRENT_VERSION: u32 = 1;

pub const DEFAULT_PORT: u16 = 4317;
pub const DEFAULT_STALE_MIN: u64 = 10;
/// Total silence before an idle session is removed from the list. It stays
/// visibly greyed from `DEFAULT_STALE_MIN` until this — long enough to persist
/// rather than blink out, short enough to eventually clear a dead session whose
/// terminal never fired `SessionEnd`.
pub const DEFAULT_IDLE_DROP_MIN: u64 = 60;

/// Built-in notification sounds (macOS system sound names under
/// `/System/Library/Sounds`). The settings UI offers this set.
pub const SOUNDS: &[&str] = &["Ping", "Glass", "Submarine", "Funk", "Pop", "Hero"];

/// Per-state notification preference.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct StateNotify {
    pub enabled: bool,
    pub sound: bool,
    pub sound_name: String,
}

impl StateNotify {
    fn new(enabled: bool, sound_name: &str) -> Self {
        StateNotify {
            enabled,
            sound: false,
            sound_name: sound_name.to_string(),
        }
    }
}

impl Default for StateNotify {
    fn default() -> Self {
        StateNotify::new(false, "Ping")
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub port: u16,
    pub stale_timeout_min: u64,
    /// Minutes of total silence before an idle session is removed from the list
    /// entirely. Until then it stays visible, greyed ("No response"). Always
    /// `>= stale_timeout_min` (normalized in `sanitized`).
    pub idle_drop_min: u64,
    pub launch_on_login: bool,
    /// Notify when a session goes idle/stale. Off by default (spec: never notify
    /// on stale-drop unless enabled).
    pub notify_idle: bool,
    /// Suppress a transition notification when that session's terminal window is
    /// already frontmost — you're looking right at it. On by default. Falls back
    /// to firing whenever the terminal can't be resolved, so a Needs-you alert is
    /// never silently dropped. App/window level only: it can't tell which *tab*
    /// of a multiplexed terminal or IDE is focused (see settings copy / docs).
    pub notify_unfocused_only: bool,
    /// Active theme id (mirrors src/themes). The palette itself lives in the
    /// frontend; the backend only stores the chosen id and reacts to the palette
    /// the webview pushes via `set_tray_palette`.
    pub theme: String,
    pub needs_you: StateNotify,
    pub working: StateNotify,
    pub ready: StateNotify,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            version: CURRENT_VERSION,
            port: DEFAULT_PORT,
            stale_timeout_min: DEFAULT_STALE_MIN,
            idle_drop_min: DEFAULT_IDLE_DROP_MIN,
            launch_on_login: false,
            notify_idle: false,
            notify_unfocused_only: true,
            theme: "classic".to_string(),
            // Spec defaults: Red on (sound off); Orange/Green off.
            needs_you: StateNotify::new(true, "Ping"),
            working: StateNotify::new(false, "Pop"),
            ready: StateNotify::new(false, "Glass"),
        }
    }
}

impl Config {
    /// Clamp/normalize values arriving from the UI or an older file, and stamp
    /// the current schema version.
    pub fn sanitized(mut self) -> Self {
        // Stay out of the privileged range; fall back to the default port.
        if self.port < 1024 {
            self.port = DEFAULT_PORT;
        }
        if self.stale_timeout_min == 0 {
            self.stale_timeout_min = DEFAULT_STALE_MIN;
        }
        // An idle session must be greyed before it's dropped, so the drop window
        // can never be shorter than the stale timeout.
        if self.idle_drop_min < self.stale_timeout_min {
            self.idle_drop_min = self.stale_timeout_min;
        }
        if self.theme.trim().is_empty() {
            self.theme = "classic".to_string();
        }
        self.version = CURRENT_VERSION;
        self
    }
}

/// Load config from the store, or defaults if absent/unreadable.
pub fn load(app: &AppHandle) -> Config {
    if let Ok(store) = app.store(STORE_FILE) {
        if let Some(v) = store.get(CONFIG_KEY) {
            if let Ok(cfg) = serde_json::from_value::<Config>(v) {
                return cfg.sanitized();
            }
        }
    }
    Config::default()
}

/// Persist config to the store.
pub fn save(app: &AppHandle, cfg: &Config) -> Result<(), String> {
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    let v = serde_json::to_value(cfg).map_err(|e| e.to_string())?;
    store.set(CONFIG_KEY, v);
    store.save().map_err(|e| e.to_string())
}
