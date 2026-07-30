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
/// Days an observation record survives before `prune` drops it.
pub const DEFAULT_OBSERVE_RETAIN_DAYS: u64 = 30;
/// Default minimum cluster size before an observed opening is offered as a
/// filter proposal.
pub const DEFAULT_PROPOSE_THRESHOLD: u32 = 3;
/// Floor for `propose_threshold`, enforced in [`Config::sanitized`] and
/// re-enforced in `proposals::build` — measured leakage on the research
/// corpus was 26 human patterns at 1, 3 at 2, and 0 at 3.
pub const MIN_PROPOSE_THRESHOLD: u32 = 3;

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
    /// Rules that hide non-interactive / machine-spawned sessions (e.g. headless
    /// `claude --print` agents launched by third-party tooling) from the widget
    /// and tray rollup.
    ///
    /// **Empty by default** — Session Signals hides nothing until you ask it to.
    /// See `docs/IGNORE-RULES.md` for ready-made patterns. Deserialized leniently
    /// so a rule kind from a newer/older build is dropped rather than aborting the
    /// whole config parse (which would reset every unrelated setting).
    #[serde(default, deserialize_with = "crate::ignore::deserialize_lenient")]
    pub ignore_rules: Vec<crate::ignore::Matcher>,
    /// Openings the user has declared their own — outranks `ignore_rules` and
    /// is never observed (see `observe.rs`). **Empty by default**, same
    /// rationale as `ignore_rules`: no shipped pattern names a specific tool.
    /// Same lenient deserializer: an unrecognized matcher kind is dropped
    /// rather than aborting the whole config parse.
    #[serde(default, deserialize_with = "crate::ignore::deserialize_lenient")]
    pub never_hide: Vec<crate::ignore::Matcher>,
    /// User-configured additions to the built-in marker registry
    /// (`markers::BUILTIN_HUMAN`). **Additive only** — an entry colliding
    /// with a built-in prefix is dropped (see `markers::Registry::new`).
    /// Ordinary `#[serde(default)]`, not the lenient deserializer: this is a
    /// plain struct shape, not a tagged enum, so an unparseable entry is a
    /// genuine config error.
    #[serde(default)]
    pub markers: Vec<crate::markers::MarkerRule>,
    /// Whether Session Signals reads session openings to look for repeating
    /// patterns (salted-hash counts only — see `observe.rs`). On by default:
    /// the eventual filter-proposal surface presumes observation runs.
    #[serde(default = "default_observe_enabled")]
    pub observe_enabled: bool,
    /// Days an observation record is kept before being pruned. `0` sanitizes
    /// to [`DEFAULT_OBSERVE_RETAIN_DAYS`] — there's no "never" here.
    #[serde(default)]
    pub observe_retain_days: u64,
    /// Minimum cluster size before an observed opening is offered as a
    /// filter proposal. **Floored at [`MIN_PROPOSE_THRESHOLD`] in
    /// `sanitized()`, not in the UI** — measured leakage on the research
    /// corpus was 26 human patterns at 1, 3 at 2, and 0 at 3, and a UI-only
    /// default is bypassable by hand-editing this file.
    #[serde(default = "default_propose_threshold")]
    pub propose_threshold: u32,
}

fn default_observe_enabled() -> bool {
    true
}

fn default_propose_threshold() -> u32 {
    DEFAULT_PROPOSE_THRESHOLD
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
            ignore_rules: crate::ignore::IgnoreRules::defaults(),
            never_hide: Vec::new(),
            markers: Vec::new(),
            observe_enabled: true,
            observe_retain_days: DEFAULT_OBSERVE_RETAIN_DAYS,
            propose_threshold: DEFAULT_PROPOSE_THRESHOLD,
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
        if self.observe_retain_days == 0 {
            self.observe_retain_days = DEFAULT_OBSERVE_RETAIN_DAYS;
        }
        if self.propose_threshold < MIN_PROPOSE_THRESHOLD {
            self.propose_threshold = MIN_PROPOSE_THRESHOLD;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A config written by a build that predates this plan (no `never_hide`,
    /// `markers`, `observe_enabled`, `observe_retain_days` keys) must still
    /// load, with the new fields filling in from their defaults rather than
    /// aborting the whole parse.
    #[test]
    fn existing_config_json_loads_with_new_defaults() {
        let json = serde_json::json!({
            "version": 1,
            "port": 4317,
            "stale_timeout_min": 10,
            "idle_drop_min": 60,
            "launch_on_login": false,
            "notify_idle": false,
            "notify_unfocused_only": true,
            "theme": "classic",
            "needs_you": { "enabled": true, "sound": false, "sound_name": "Ping" },
            "working": { "enabled": false, "sound": false, "sound_name": "Pop" },
            "ready": { "enabled": false, "sound": false, "sound_name": "Glass" },
            "ignore_rules": []
        });
        let cfg: Config = serde_json::from_value(json).expect("old config must still parse");
        let cfg = cfg.sanitized();
        assert!(cfg.never_hide.is_empty());
        assert!(cfg.markers.is_empty());
        assert!(cfg.observe_enabled);
        assert_eq!(cfg.observe_retain_days, DEFAULT_OBSERVE_RETAIN_DAYS);
        assert_eq!(cfg.propose_threshold, DEFAULT_PROPOSE_THRESHOLD);
    }

    #[test]
    fn zero_observe_retain_days_sanitizes_to_default() {
        let mut cfg = Config {
            observe_retain_days: 0,
            ..Config::default()
        };
        cfg = cfg.sanitized();
        assert_eq!(cfg.observe_retain_days, DEFAULT_OBSERVE_RETAIN_DAYS);
    }

    #[test]
    fn propose_threshold_below_floor_is_clamped() {
        for (input, expected) in [(0, 3), (1, 3), (5, 5)] {
            let cfg = Config {
                propose_threshold: input,
                ..Config::default()
            }
            .sanitized();
            assert_eq!(cfg.propose_threshold, expected, "input {input}");
        }
    }
}
