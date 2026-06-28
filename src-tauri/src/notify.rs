//! Notification engine: fire OS notifications on state *transitions* only.
//!
//! `lib.rs` calls `fire` whenever the engine reports a session changed state.
//! Because we only ever act on a real transition (the engine emits one only
//! when `from != to`), a permission prompt that simply *stays* in Needs-you
//! never re-notifies. On top of that we debounce identical (session, state)
//! transitions within a short window to collapse rapid storms.

use crate::config::Config;
use crate::engine::{State, Transition};
use crate::tray::TrayPalette;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

/// Notification icons render larger than the tray dot so they stay crisp in the
/// OS notification, which displays at a higher resolution than the menu bar.
const NOTIF_ICON_SIZE: u32 = 64;

fn state_slug(state: State) -> &'static str {
    match state {
        State::NeedsYou => "needs_you",
        State::Working => "working",
        State::Ready => "ready",
    }
}

/// Path of the themed notification icon for `state` (in the app cache dir).
pub fn icon_path(app: &AppHandle, state: State) -> Option<PathBuf> {
    let dir = app.path().app_cache_dir().ok()?;
    Some(dir.join(format!("beacon-notif-{}.png", state_slug(state))))
}

/// (Re)render the three per-state notification icons from the active palette into
/// the cache dir. Called at startup and on every theme change, so notification
/// icons track the theme with no bundled assets. Best-effort: any IO/encode
/// error is ignored (notifications simply fall back to the app icon).
///
/// Platform note: macOS always shows the *app* icon on notifications regardless
/// of a per-notification icon, so these themed icons are honored on Windows and
/// Linux; on macOS the body text still conveys the state.
pub fn render_icons(app: &AppHandle, palette: &TrayPalette) {
    let dir = match app.path().app_cache_dir() {
        Ok(d) => d,
        Err(_) => return,
    };
    let _ = std::fs::create_dir_all(&dir);
    for (state, rgb, shape) in [
        (State::NeedsYou, palette.needs_you, crate::tray::Shape::Square),
        (State::Working, palette.working, crate::tray::Shape::Dot),
        (State::Ready, palette.ready, crate::tray::Shape::Check),
    ] {
        let rgba = crate::tray::render_glyph_rgba(shape, rgb, NOTIF_ICON_SIZE);
        if let (Some(png), Some(path)) =
            (crate::tray::encode_png(&rgba, NOTIF_ICON_SIZE), icon_path(app, state))
        {
            let _ = std::fs::write(path, png);
        }
    }
}

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

        // Design copy: title is "{project} <verb>", body is a short context line.
        // The project is the folder portion of the label (drop the "(branch)").
        // A richer body ("waiting for permission" vs "asked a question") would
        // need notification_type plumbed through the engine — out of scope for a
        // presentation pass — so the body is a sensible generic per state.
        let project = t.label.split(" (").next().unwrap_or(&t.label).to_string();
        let (title, body) = match t.to {
            State::NeedsYou => (format!("{project} needs you"), "Waiting for your input."),
            State::Working => (format!("{project} is working"), "Running — don’t interrupt."),
            State::Ready => (format!("{project} is ready"), "Finished — your turn."),
        };
        let sound = if pref.sound {
            Some(pref.sound_name.clone())
        } else {
            None
        };
        // Attach the themed icon for the target state if it has been rendered.
        let icon = icon_path(app, t.to).filter(|p| p.exists());

        // Show on a detached thread: the OS Notification Center can be slow to
        // return, and we never want that to stall the event worker (which would
        // delay other sessions' state updates).
        let app = app.clone();
        std::thread::spawn(move || {
            let mut builder = app.notification().builder().title(title).body(body);
            if let Some(name) = sound {
                builder = builder.sound(name);
            }
            if let Some(path) = icon {
                builder = builder.icon(path.to_string_lossy().to_string());
            }
            let _ = builder.show();
        });
    }
}
