//! The floating widget window.
//!
//! A frameless, transparent, always-on-top `WebviewWindow` that renders one row
//! per live session. It is a pure renderer — all state arrives via the
//! `sessions-updated` event emitted by `lib.rs`. This module owns only the
//! window's *chrome*: creation, positioning (persisted + multi-monitor clamped),
//! show/hide, and the view preferences (compact / opacity / visible) that
//! survive restarts via `tauri-plugin-store`.

use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, LogicalSize, Manager, PhysicalPosition, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_store::StoreExt;

/// Window label; also the route discriminator the React entry point reads.
pub const WIDGET_LABEL: &str = "widget";
/// Store file (lives in the app config dir).
const STORE_FILE: &str = "beacon.json";

const KEY_POS: &str = "widget.position";
const KEY_VISIBLE: &str = "widget.visible";
const KEY_COMPACT: &str = "widget.compact";
const KEY_OPACITY: &str = "widget.opacity";
/// The expanded size to restore to when un-minifying. Captured the moment the
/// user collapses, so "original size" means whatever they last had open.
const KEY_EXPANDED_W: &str = "widget.expandedWidth";
const KEY_EXPANDED_H: &str = "widget.expandedHeight";

const DEFAULT_W: f64 = 320.0;
const DEFAULT_H: f64 = 420.0;
/// Collapsed height: the headerless pill is just a single non-wrapping row of
/// glyphs (`Widget.css` `.wPill`), `flex: 1` and vertically centered, so a few
/// px of slack only adds centering — it never clips.
const COMPACT_H: f64 = 48.0;
/// Floor for the collapsed pill's width (a one-glyph pill) and the expanded
/// view's minimums. Applied per-mode via `set_min_size` so the pill can shrink
/// far below the expanded layout's usable width.
const COMPACT_MIN_W: f64 = 56.0;
const EXPANDED_MIN_W: f64 = 200.0;
const EXPANDED_MIN_H: f64 = 120.0;

/// View preferences shared with the webview. The window position is persisted
/// separately (it changes on drag, not via the UI).
#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct WidgetPrefs {
    pub compact: bool,
    pub opacity: f64,
    pub visible: bool,
}

impl Default for WidgetPrefs {
    fn default() -> Self {
        WidgetPrefs {
            compact: false,
            opacity: 0.95,
            visible: true,
        }
    }
}

/// Create the widget window at startup and show it only if it was visible when
/// Beacon last ran. The window always exists (so it can receive events); hiding
/// is just a visibility toggle.
pub fn init(app: &AppHandle) -> tauri::Result<()> {
    let prefs = load_prefs(app);
    let window = build_window(app)?;
    position_window(app, &window);
    // Restore the last mode's size (built at the expanded default; collapse or
    // grow to the persisted state before the first paint).
    apply_size(app, &window, prefs.compact);
    if prefs.visible {
        window.show()?;
    }
    Ok(())
}

/// Build the frameless/transparent/always-on-top window (initially hidden so we
/// can place it before the first paint).
fn build_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let window = WebviewWindowBuilder::new(app, WIDGET_LABEL, WebviewUrl::App("index.html".into()))
        .title("Beacon")
        .inner_size(DEFAULT_W, DEFAULT_H)
        // Starting min for the expanded view; `apply_size` swaps this per mode
        // (a collapsed pill needs a far smaller floor — see COMPACT_MIN_W).
        .min_inner_size(EXPANDED_MIN_W, EXPANDED_MIN_H)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(true)
        .shadow(false)
        .visible(false)
        .build()?;

    // Persist position on drag; closing the widget just hides it (Beacon keeps
    // running in the tray).
    let app2 = app.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::Moved(pos) => save_position(&app2, pos.x, pos.y),
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            hide(&app2);
        }
        _ => {}
    });

    Ok(window)
}

/// Restore the saved position, clamped to a currently-visible monitor; if there
/// is no saved position (or it's now off-screen) fall back to the top-right of
/// the primary display.
fn position_window(app: &AppHandle, window: &WebviewWindow) {
    let target = match saved_position(app) {
        Some(pos) if is_on_screen(window, pos) => pos,
        _ => default_position(window),
    };
    let _ = window.set_position(target);
}

/// Show the widget (creating it if it was somehow torn down) and remember it.
pub fn show(app: &AppHandle) {
    match app.get_webview_window(WIDGET_LABEL) {
        Some(window) => {
            let _ = window.show();
            let _ = window.set_focus();
        }
        None => {
            if let Ok(window) = build_window(app) {
                position_window(app, &window);
                let _ = window.show();
            }
        }
    }
    set_bool(app, KEY_VISIBLE, true);
}

/// Hide the widget but keep the app (and the window object) alive.
pub fn hide(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(WIDGET_LABEL) {
        let _ = window.hide();
    }
    set_bool(app, KEY_VISIBLE, false);
}

/// Tray entry point: flip visibility.
pub fn toggle(app: &AppHandle) {
    let visible = app
        .get_webview_window(WIDGET_LABEL)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    if visible {
        hide(app);
    } else {
        show(app);
    }
}

// --- Preferences ----------------------------------------------------------

pub fn load_prefs(app: &AppHandle) -> WidgetPrefs {
    let mut prefs = WidgetPrefs::default();
    if let Ok(store) = app.store(STORE_FILE) {
        if let Some(v) = store.get(KEY_COMPACT).and_then(|v| v.as_bool()) {
            prefs.compact = v;
        }
        if let Some(v) = store.get(KEY_OPACITY).and_then(|v| v.as_f64()) {
            prefs.opacity = v.clamp(0.3, 1.0);
        }
        if let Some(v) = store.get(KEY_VISIBLE).and_then(|v| v.as_bool()) {
            prefs.visible = v;
        }
    }
    prefs
}

/// Toggle the collapsed/minified view. Besides persisting the preference this
/// resizes the actual window: collapsing remembers the current expanded height
/// then shrinks to `COMPACT_H`; expanding restores the remembered height. The
/// width is left untouched, and manual resize is disabled while collapsed.
pub fn set_compact(app: &AppHandle, compact: bool) {
    set_bool(app, KEY_COMPACT, compact);
    if let Some(window) = app.get_webview_window(WIDGET_LABEL) {
        if compact {
            // Capture the size we're collapsing from (the window is still
            // expanded at this point) so expanding restores exactly that.
            let (w, h) = logical_inner(&window);
            store_f64(app, KEY_EXPANDED_W, w);
            store_f64(app, KEY_EXPANDED_H, h);
        }
        apply_size(app, &window, compact);
    }
}

/// Set the window to the size for `compact` without mutating the saved expanded
/// size (used both by the toggle, after it has captured the size, and at
/// startup). Collapsing keeps the current width as a provisional value — the
/// webview measures its content and calls [`set_compact_width`] to hug it.
/// Manual resize and the size floor are adjusted per mode.
fn apply_size(app: &AppHandle, window: &WebviewWindow, compact: bool) {
    if compact {
        // Lower the floor first: set_size is clamped to the min, and a pill is
        // far narrower/shorter than the expanded minimum.
        let _ = window.set_min_size(Some(LogicalSize::new(COMPACT_MIN_W, COMPACT_H)));
        let (w, _) = logical_inner(window);
        let _ = window.set_size(LogicalSize::new(w, COMPACT_H));
        let _ = window.set_resizable(false);
    } else {
        let _ = window.set_min_size(Some(LogicalSize::new(EXPANDED_MIN_W, EXPANDED_MIN_H)));
        let w = load_f64(app, KEY_EXPANDED_W).unwrap_or(DEFAULT_W);
        let h = load_f64(app, KEY_EXPANDED_H).unwrap_or(DEFAULT_H);
        let _ = window.set_size(LogicalSize::new(w, h));
        let _ = window.set_resizable(true);
    }
}

/// Narrow the collapsed pill to the content width the webview measured, so it
/// hugs its glyphs instead of keeping the expanded width. No-op unless the
/// widget is currently collapsed (a late call arriving after the user expanded
/// is ignored). Width is clamped to a sane pill range.
pub fn set_compact_width(app: &AppHandle, width: f64) {
    if !load_prefs(app).compact {
        return;
    }
    if let Some(window) = app.get_webview_window(WIDGET_LABEL) {
        let w = width.clamp(COMPACT_MIN_W, DEFAULT_W);
        let _ = window.set_size(LogicalSize::new(w, COMPACT_H));
    }
}

/// The window's current inner size in logical (DPI-independent) px. Falls back
/// to the defaults if the platform query fails.
fn logical_inner(window: &WebviewWindow) -> (f64, f64) {
    let scale = window.scale_factor().unwrap_or(1.0);
    match window.inner_size() {
        Ok(s) => (s.width as f64 / scale, s.height as f64 / scale),
        Err(_) => (DEFAULT_W, DEFAULT_H),
    }
}

fn store_f64(app: &AppHandle, key: &str, v: f64) {
    if let Ok(store) = app.store(STORE_FILE) {
        store.set(key, v);
        let _ = store.save();
    }
}

fn load_f64(app: &AppHandle, key: &str) -> Option<f64> {
    let store = app.store(STORE_FILE).ok()?;
    store.get(key).and_then(|v| v.as_f64())
}

pub fn set_opacity(app: &AppHandle, opacity: f64) {
    if let Ok(store) = app.store(STORE_FILE) {
        store.set(KEY_OPACITY, opacity.clamp(0.3, 1.0));
        let _ = store.save();
    }
}

fn set_bool(app: &AppHandle, key: &str, value: bool) {
    if let Ok(store) = app.store(STORE_FILE) {
        store.set(key, value);
        let _ = store.save();
    }
}

// --- Position persistence + multi-monitor clamping ------------------------

fn save_position(app: &AppHandle, x: i32, y: i32) {
    if let Ok(store) = app.store(STORE_FILE) {
        store.set(KEY_POS, serde_json::json!({ "x": x, "y": y }));
        let _ = store.save();
    }
}

fn saved_position(app: &AppHandle) -> Option<PhysicalPosition<i32>> {
    let store = app.store(STORE_FILE).ok()?;
    let v = store.get(KEY_POS)?;
    let x = v.get("x")?.as_i64()? as i32;
    let y = v.get("y")?.as_i64()? as i32;
    Some(PhysicalPosition::new(x, y))
}

/// True if the window's title strip would land on some connected monitor. We
/// require a small inset so the (frameless) drag header stays grabbable even if
/// the saved spot is right at a screen edge.
fn is_on_screen(window: &WebviewWindow, pos: PhysicalPosition<i32>) -> bool {
    let monitors = match window.available_monitors() {
        Ok(m) => m,
        Err(_) => return false,
    };
    monitors.iter().any(|m| {
        let mp = m.position();
        let ms = m.size();
        let x0 = mp.x;
        let y0 = mp.y;
        let x1 = x0 + ms.width as i32;
        let y1 = y0 + ms.height as i32;
        pos.x >= x0 && pos.x <= x1 - 40 && pos.y >= y0 && pos.y <= y1 - 20
    })
}

/// Top-right corner of the primary monitor, inset by a margin.
fn default_position(window: &WebviewWindow) -> PhysicalPosition<i32> {
    if let Ok(Some(mon)) = window.primary_monitor() {
        let mp = mon.position();
        let ms = mon.size();
        let scale = mon.scale_factor();
        let w = (DEFAULT_W * scale) as i32;
        let margin = (24.0 * scale) as i32;
        PhysicalPosition::new(mp.x + ms.width as i32 - w - margin, mp.y + margin)
    } else {
        PhysicalPosition::new(100, 100)
    }
}
