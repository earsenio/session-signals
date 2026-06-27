//! Notification engine: fire OS notifications on state *transitions* only.
//!
//! `lib.rs` calls `fire` whenever the engine reports a session changed state.
//! Because we only ever act on a real transition (the engine emits one only
//! when `from != to`), a permission prompt that simply *stays* in Needs-you
//! never re-notifies. On top of that we debounce identical (session, state)
//! transitions within a short window to collapse rapid storms.

use crate::config::Config;
use crate::engine::{State, Transition};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

/// Suppress an identical (session, target-state) notification fired within this
/// window.
const DEBOUNCE: Duration = Duration::from_millis(1500);

pub struct Notifier {
    /// Last time we fired for a given `session:state` key.
    recent: Mutex<HashMap<String, Instant>>,
}

impl Notifier {
    pub fn new() -> Self {
        Notifier {
            recent: Mutex::new(HashMap::new()),
        }
    }

    /// Fire a notification for a transition if the target state's preference is
    /// enabled and it isn't a debounced repeat.
    pub fn fire(&self, app: &AppHandle, cfg: &Config, t: &Transition) {
        let pref = match t.to {
            State::NeedsYou => &cfg.needs_you,
            State::Working => &cfg.working,
            State::Ready => &cfg.ready,
        };
        if !pref.enabled {
            return;
        }

        // Debounce identical (session, to) transitions.
        let key = format!("{}:{:?}", t.session_id, t.to);
        {
            let now = Instant::now();
            let mut recent = self.recent.lock().expect("notifier mutex poisoned");
            if let Some(&last) = recent.get(&key) {
                if now.duration_since(last) < DEBOUNCE {
                    return;
                }
            }
            recent.insert(key, now);
            // Opportunistic cleanup so the map can't grow unbounded.
            recent.retain(|_, &mut ts| now.duration_since(ts) < Duration::from_secs(60));
        }

        let body = match t.to {
            State::NeedsYou => format!("{} needs you", t.label),
            State::Working => format!("{} is working", t.label),
            State::Ready => format!("{} is ready", t.label),
        };

        let mut builder = app.notification().builder().title("Beacon").body(body);
        if pref.sound {
            builder = builder.sound(pref.sound_name.clone());
        }
        let _ = builder.show();
    }
}
