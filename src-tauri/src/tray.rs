//! Tray icon + menu. The icon color reflects the engine rollup; the menu drives
//! hook install/uninstall, opens the settings window, and quits.

use crate::engine::Rollup;
use crate::hooks;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconId};
use tauri::{AppHandle, Emitter, Manager};

const TRAY_ID: &str = "beacon-tray";

/// Decode the colored PNG for a given rollup. Icons are bundled at compile time.
fn icon_for(rollup: Rollup) -> Image<'static> {
    let bytes: &[u8] = match rollup {
        Rollup::Red => include_bytes!("../icons/beacon-red.png"),
        Rollup::Orange => include_bytes!("../icons/beacon-orange.png"),
        Rollup::Green => include_bytes!("../icons/beacon-green.png"),
        Rollup::Grey => include_bytes!("../icons/beacon-grey.png"),
    };
    Image::from_bytes(bytes).expect("bundled tray icon is a valid PNG")
}

fn tooltip_for(rollup: Rollup) -> &'static str {
    match rollup {
        Rollup::Red => "Beacon — a session needs you",
        Rollup::Orange => "Beacon — working",
        Rollup::Green => "Beacon — ready",
        Rollup::Grey => "Beacon — no live sessions",
    }
}

/// Build the tray icon and menu. Starts grey (no sessions yet).
pub fn build(app: &AppHandle, port: u16) -> tauri::Result<()> {
    let install = MenuItem::with_id(app, "install", "Install Claude Code hooks", true, None::<&str>)?;
    let uninstall =
        MenuItem::with_id(app, "uninstall", "Uninstall hooks", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Open Beacon…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Beacon", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;

    let menu = Menu::with_items(app, &[&install, &uninstall, &sep1, &settings, &sep2, &quit])?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon_for(Rollup::Grey))
        .tooltip(tooltip_for(Rollup::Grey))
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(move |app, event| handle_menu(app, event.id().as_ref(), port))
        .build(app)?;

    Ok(())
}

/// Update the tray icon + tooltip to reflect the current rollup.
pub fn set_rollup(app: &AppHandle, rollup: Rollup) {
    if let Some(tray) = app.tray_by_id(&TrayIconId::new(TRAY_ID)) {
        let _ = tray.set_icon(Some(icon_for(rollup)));
        let _ = tray.set_tooltip(Some(tooltip_for(rollup)));
    }
}

fn handle_menu(app: &AppHandle, id: &str, port: u16) {
    match id {
        "install" => {
            let msg = match hooks::install(port) {
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

fn show_settings(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Surface a short status message to the settings window (if open).
fn toast(app: &AppHandle, message: &str) {
    let _ = app.emit("beacon://toast", message);
    eprintln!("[beacon] {message}");
}
