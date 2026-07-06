//! Tray icon + menu. The icon is **rendered from the active theme's palette**
//! (pushed up from the webview via `set_tray_palette`), not loaded from per-theme
//! image files — so adding a theme needs no assets. The icon color reflects the
//! engine rollup; the menu drives hook install/uninstall, opens settings, quits.

use crate::engine::Rollup;
use crate::glyph::{render_glyph_rgba, shape_for_rollup};
use crate::hooks;
use crate::windows;
use serde::{Deserialize, Serialize};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconId};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_store::StoreExt;

const TRAY_ID: &str = "beacon-tray";
const STORE_FILE: &str = "beacon.json";
const PALETTE_KEY: &str = "tray.palette";
/// Render size of the tray dot (square RGBA). The OS scales it to the menu-bar
/// height; 32px stays crisp when scaled down on both macOS and Windows.
const TRAY_SIZE: u32 = 32;

/// Colors pushed from the active theme. RGB triples (0–255). `rollup` colors
/// drive the tray; `state` colors let the notifier render matching glyphs from
/// the same source. Shapes are fixed per state (see `Shape`), not stored here.
#[derive(Clone, Copy, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct TrayPalette {
    pub red: [u8; 3],
    pub orange: [u8; 3],
    pub green: [u8; 3],
    pub grey: [u8; 3],
    pub needs_you: [u8; 3],
    pub working: [u8; 3],
    pub ready: [u8; 3],
}

impl Default for TrayPalette {
    /// The `classic` theme — the backend's fallback before the webview pushes the
    /// persisted choice, so the tray is never blank or wrongly-colored at launch.
    fn default() -> Self {
        TrayPalette {
            red: [244, 89, 94],
            orange: [245, 167, 66],
            green: [70, 201, 139],
            grey: [124, 130, 141],
            needs_you: [244, 89, 94],
            working: [245, 167, 66],
            ready: [70, 201, 139],
        }
    }
}

impl TrayPalette {
    fn rollup_rgb(&self, rollup: Rollup) -> [u8; 3] {
        match rollup {
            Rollup::Red => self.red,
            Rollup::Orange => self.orange,
            Rollup::Green => self.green,
            Rollup::Grey => self.grey,
        }
    }
}

/// Build the tray glyph for a rollup from the given palette.
fn icon_for(palette: &TrayPalette, rollup: Rollup) -> Image<'static> {
    let buf = render_glyph_rgba(
        shape_for_rollup(rollup),
        palette.rollup_rgb(rollup),
        TRAY_SIZE,
    );
    Image::new_owned(buf, TRAY_SIZE, TRAY_SIZE)
}

fn tooltip_for(rollup: Rollup) -> &'static str {
    match rollup {
        Rollup::Red => "Session Signals — a session needs you",
        Rollup::Orange => "Session Signals — working",
        Rollup::Green => "Session Signals — ready",
        Rollup::Grey => "Session Signals — no live sessions",
    }
}

/// Load the last-pushed palette from the store, or the classic default. Reading
/// it at build time means a non-default theme survives a restart with no flash.
pub fn load_palette(app: &AppHandle) -> TrayPalette {
    if let Ok(store) = app.store(STORE_FILE) {
        if let Some(v) = store.get(PALETTE_KEY) {
            if let Ok(p) = serde_json::from_value::<TrayPalette>(v) {
                return p;
            }
        }
    }
    TrayPalette::default()
}

/// Persist the active palette so the tray restyles instantly on next launch.
pub fn save_palette(app: &AppHandle, palette: &TrayPalette) {
    if let Ok(store) = app.store(STORE_FILE) {
        if let Ok(v) = serde_json::to_value(palette) {
            store.set(PALETTE_KEY, v);
            let _ = store.save();
        }
    }
}

/// Build the tray icon and menu. Starts grey (no sessions yet), using `palette`.
pub fn build(app: &AppHandle, palette: &TrayPalette) -> tauri::Result<()> {
    let widget = MenuItem::with_id(app, "widget", "Show / hide widget", true, None::<&str>)?;
    let install = MenuItem::with_id(
        app,
        "install",
        "Install Claude Code hooks",
        true,
        None::<&str>,
    )?;
    let uninstall = MenuItem::with_id(app, "uninstall", "Uninstall hooks", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Open Session Signals…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Session Signals", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let sep3 = PredefinedMenuItem::separator(app)?;

    let menu = Menu::with_items(
        app,
        &[
            &widget, &sep1, &install, &uninstall, &sep2, &settings, &sep3, &quit,
        ],
    )?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon_for(palette, Rollup::Grey))
        // Colored dot, not a monochrome template.
        .icon_as_template(false)
        .tooltip(tooltip_for(Rollup::Grey))
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(move |app, event| handle_menu(app, event.id().as_ref()))
        .build(app)?;

    Ok(())
}

/// Update the tray icon + tooltip to reflect the current rollup, using `palette`.
pub fn set_rollup(app: &AppHandle, rollup: Rollup, palette: &TrayPalette) {
    if let Some(tray) = app.tray_by_id(&TrayIconId::new(TRAY_ID)) {
        let _ = tray.set_icon(Some(icon_for(palette, rollup)));
        let _ = tray.set_icon_as_template(false);
        let _ = tray.set_tooltip(Some(tooltip_for(rollup)));
    }
}

fn handle_menu(app: &AppHandle, id: &str) {
    // Use the live, configured port (it can change in settings).
    let port = crate::current_port(app);
    match id {
        "widget" => windows::toggle(app),
        "install" => {
            let msg = match crate::install_beacon_hooks(app) {
                Ok(path) => format!("Hooks installed in {}", path.display()),
                Err(e) => format!("Install failed: {e}"),
            };
            toast(app, &msg);
            show_settings(app);
        }
        "uninstall" => {
            let msg = match hooks::uninstall(port) {
                Ok(path) => format!("Hooks removed from {}", path.display()),
                Err(e) => format!("Uninstall failed: {e}"),
            };
            toast(app, &msg);
            show_settings(app);
        }
        "settings" => show_settings(app),
        "quit" => app.exit(0),
        _ => {}
    }
}

pub(crate) fn show_settings(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("settings") {
        // Dev-mode self-heal. In `tauri dev` the settings window points at the
        // Vite dev server and is kept hidden between opens. If that webview is
        // ever left holding a dead page — Vite was down when it first tried to
        // load, Vite restarted, or an HMR full reload fired while it was hidden
        // — it presents blank when next shown (and can take the always-on-top
        // widget's transparent webview down with it when both are stale: the
        // "settings hides the widget" report). `reload()` is not enough: it only
        // re-runs the *current* document, so a webview whose initial navigation
        // failed has nothing live to reload and stays blank. Re-navigating to
        // the configured dev URL forces a fresh fetch, recovering even a
        // never-loaded webview. Release builds serve static bundled assets that
        // can't go stale, so this is compiled out there — no flash in production.
        #[cfg(debug_assertions)]
        if let Some(dev_url) = app.config().build.dev_url.clone() {
            let _ = window.navigate(dev_url);
        } else {
            let _ = window.reload();
        }
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Surface a short status message to the settings window (if open).
fn toast(app: &AppHandle, message: &str) {
    let _ = app.emit("beacon://toast", message);
    eprintln!("[beacon] {message}");
}
