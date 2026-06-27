//! Beacon — desktop status indicator for Claude Code.
//!
//! The Rust shell owns the source of truth (the state engine), the localhost
//! hook listener, and the tray. The webview (settings window) is a thin
//! renderer: it asks for a snapshot and listens for updates. State flows one
//! way — engine → events → UI.

pub mod engine;
pub mod hooks;
mod listener;
mod tray;

use engine::{Engine, Rollup, SessionView};
use serde::Serialize;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};

/// Defaults (user-overridable in a later phase).
const PORT: u16 = 4317;
const STALE_TIMEOUT_MIN: u64 = 10;
const GRACE_SECS: u64 = 30;
/// How often the background sweep checks for stale sessions.
const SWEEP_INTERVAL_SECS: u64 = 15;

/// Shared application state, managed by Tauri.
struct AppState {
    engine: Mutex<Engine>,
    port: u16,
}

/// What the webview receives on every update.
#[derive(Serialize, Clone)]
struct SessionsPayload {
    rollup: Rollup,
    sessions: Vec<SessionView>,
}

/// Recompute rollup + snapshot, push to the tray and the webview.
fn refresh(app: &AppHandle) {
    let state = app.state::<AppState>();
    let payload = {
        let eng = state.engine.lock().expect("engine mutex poisoned");
        SessionsPayload {
            rollup: eng.rollup(),
            sessions: eng.snapshot(),
        }
    };
    tray::set_rollup(app, payload.rollup);
    let _ = app.emit("beacon://sessions", payload);
}

#[tauri::command]
fn get_snapshot(state: State<AppState>) -> SessionsPayload {
    let eng = state.engine.lock().expect("engine mutex poisoned");
    SessionsPayload {
        rollup: eng.rollup(),
        sessions: eng.snapshot(),
    }
}

#[tauri::command]
fn install_hooks(state: State<AppState>) -> Result<String, String> {
    hooks::install(state.port).map(|p| p.display().to_string())
}

#[tauri::command]
fn uninstall_hooks(state: State<AppState>) -> Result<String, String> {
    hooks::uninstall(state.port).map(|p| p.display().to_string())
}

#[tauri::command]
fn hook_block(state: State<AppState>) -> String {
    hooks::hook_block_string(state.port)
}

#[tauri::command]
fn endpoint(state: State<AppState>) -> String {
    hooks::endpoint(state.port)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Single-instance must be registered first: a second launch just
        // surfaces the existing settings window instead of fighting over the
        // listener port.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("settings") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            engine: Mutex::new(Engine::new(
                Duration::from_secs(STALE_TIMEOUT_MIN * 60),
                Duration::from_secs(GRACE_SECS),
            )),
            port: PORT,
        })
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            install_hooks,
            uninstall_hooks,
            hook_block,
            endpoint
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            // Tray-only app: no dock icon / app-switcher entry on macOS.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Build the tray (starts grey).
            tray::build(&handle, PORT)?;

            // Closing the settings window hides it rather than quitting Beacon.
            if let Some(window) = app.get_webview_window("settings") {
                let w = window.clone();
                window.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = w.hide();
                    }
                });
            }

            // Start the localhost hook listener. Each event mutates the engine
            // and, if anything changed, refreshes tray + webview. The /state
            // readback closure reports the current rollup + snapshot.
            let listener_handle = handle.clone();
            let state_handle = handle.clone();
            listener::start(
                PORT,
                move |ev| {
                    let changed = {
                        let state = listener_handle.state::<AppState>();
                        let mut eng = state.engine.lock().expect("engine mutex poisoned");
                        eng.apply(&ev)
                    };
                    if changed {
                        refresh(&listener_handle);
                    }
                },
                move || {
                    let state = state_handle.state::<AppState>();
                    let eng = state.engine.lock().expect("engine mutex poisoned");
                    let payload = SessionsPayload {
                        rollup: eng.rollup(),
                        sessions: eng.snapshot(),
                    };
                    serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string())
                },
            )
            .map_err(|e| -> Box<dyn std::error::Error> {
                format!("failed to bind hook listener on 127.0.0.1:{PORT}: {e}").into()
            })?;

            // Background stale sweep.
            let sweep_handle = handle.clone();
            std::thread::Builder::new()
                .name("beacon-sweep".into())
                .spawn(move || loop {
                    std::thread::sleep(Duration::from_secs(SWEEP_INTERVAL_SECS));
                    let changed = {
                        let state = sweep_handle.state::<AppState>();
                        let mut eng = state.engine.lock().expect("engine mutex poisoned");
                        eng.sweep()
                    };
                    if changed {
                        refresh(&sweep_handle);
                    }
                })?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Beacon");
}
