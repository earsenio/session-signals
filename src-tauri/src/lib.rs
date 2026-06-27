//! Beacon — desktop status indicator for Claude Code.
//!
//! The Rust shell owns the source of truth (the state engine), the localhost
//! hook listener, the tray, and now configuration + notifications. The webviews
//! (settings + widget) are thin renderers: they ask for snapshots/config and
//! listen for updates. State flows one way — engine → events → UI.

pub mod config;
pub mod engine;
pub mod hooks;
mod listener;
mod notify;
mod tray;
mod windows;

use config::Config;
use engine::{Engine, Rollup, SessionView, Transition};
use notify::Notifier;
use serde::Serialize;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tiny_http::Server;
use windows::WidgetPrefs;

const GRACE_SECS: u64 = 30;
/// How often the background sweep checks for stale sessions.
const SWEEP_INTERVAL_SECS: u64 = 15;

/// Shared application state, managed by Tauri.
struct AppState {
    engine: Mutex<Engine>,
    config: Mutex<Config>,
    /// The currently-bound hook listener. Held so a port change can stop it
    /// (`unblock`) and swap in a new one.
    listener: Mutex<Option<Arc<Server>>>,
    notifier: Notifier,
}

/// What the webview receives on every update.
#[derive(Serialize, Clone)]
struct SessionsPayload {
    rollup: Rollup,
    sessions: Vec<SessionView>,
}

/// Recompute rollup + snapshot, push to the tray and the webviews.
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
    let _ = app.emit("sessions-updated", payload);
}

/// React to a state transition: fire a notification per the user's config.
fn on_transition(app: &AppHandle, t: &Transition) {
    let state = app.state::<AppState>();
    let cfg = state.config.lock().expect("config mutex poisoned").clone();
    state.notifier.fire(app, &cfg, t);
}

/// Build and start a listener on `port`, wiring its callbacks back into the
/// engine + notifier. Returns the server handle (or a bind error).
fn spawn_listener(app: &AppHandle, port: u16) -> std::io::Result<Arc<Server>> {
    let event_handle = app.clone();
    let state_handle = app.clone();
    listener::start(
        port,
        move |ev| {
            let outcome = {
                let state = event_handle.state::<AppState>();
                let mut eng = state.engine.lock().expect("engine mutex poisoned");
                eng.apply(&ev)
            };
            if outcome.changed {
                refresh(&event_handle);
            }
            if let Some(t) = outcome.transition {
                on_transition(&event_handle, &t);
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
}

/// The port currently configured.
fn current_port(app: &AppHandle) -> u16 {
    app.state::<AppState>()
        .config
        .lock()
        .expect("config mutex poisoned")
        .port
}

// --- Commands -------------------------------------------------------------

#[tauri::command]
fn get_snapshot(state: State<AppState>) -> SessionsPayload {
    let eng = state.engine.lock().expect("engine mutex poisoned");
    SessionsPayload {
        rollup: eng.rollup(),
        sessions: eng.snapshot(),
    }
}

#[tauri::command]
fn get_config(state: State<AppState>) -> Config {
    state.config.lock().expect("config mutex poisoned").clone()
}

/// Persist a new config and apply its side effects: live port restart (with a
/// hook reinstall to keep settings.json in sync), stale-timeout update, and
/// launch-on-login. Fallible steps (busy port, autostart) run *before* anything
/// is committed, so a failure leaves the running app untouched.
#[tauri::command]
fn set_config(app: AppHandle, new: Config) -> Result<(), String> {
    let new = new.sanitized();
    let old = get_config(app.state::<AppState>());

    // 1. Port changed → bind the new listener first so a busy port fails fast.
    let new_server = if new.port != old.port {
        Some(
            spawn_listener(&app, new.port)
                .map_err(|e| format!("port {} is busy or unavailable: {e}", new.port))?,
        )
    } else {
        None
    };

    // 2. Launch-on-login (fallible). If it errors, discard the freshly-bound
    //    listener and leave everything as it was.
    if new.launch_on_login != old.launch_on_login {
        if let Err(e) = set_autostart(&app, new.launch_on_login) {
            if let Some(server) = &new_server {
                server.unblock();
            }
            return Err(e);
        }
    }

    // 3. Commit the listener swap (old port stops; new one is now live).
    if let Some(server) = new_server {
        {
            let state = app.state::<AppState>();
            let mut guard = state.listener.lock().expect("listener mutex poisoned");
            if let Some(old_server) = guard.take() {
                old_server.unblock();
            }
            *guard = Some(server);
        }
        // Keep settings.json pointed at the new port — but only if Beacon's
        // hooks are actually installed (don't install just because port changed).
        if hooks::is_installed() {
            hooks::install(new.port)?;
        }
    }

    // 4. Stale timeout.
    if new.stale_timeout_min != old.stale_timeout_min {
        app.state::<AppState>()
            .engine
            .lock()
            .expect("engine mutex poisoned")
            .set_stale_timeout(Duration::from_secs(new.stale_timeout_min * 60));
    }

    // 5. Persist + update live config.
    config::save(&app, &new)?;
    *app.state::<AppState>()
        .config
        .lock()
        .expect("config mutex poisoned") = new;
    Ok(())
}

#[tauri::command]
fn install_hooks(app: AppHandle) -> Result<String, String> {
    hooks::install(current_port(&app)).map(|p| p.display().to_string())
}

#[tauri::command]
fn uninstall_hooks(app: AppHandle) -> Result<String, String> {
    hooks::uninstall(current_port(&app)).map(|p| p.display().to_string())
}

#[tauri::command]
fn hooks_installed() -> bool {
    hooks::is_installed()
}

#[tauri::command]
fn hook_block(app: AppHandle) -> String {
    hooks::hook_block_string(current_port(&app))
}

#[tauri::command]
fn endpoint(app: AppHandle) -> String {
    hooks::endpoint(current_port(&app))
}

// --- Widget commands (called from the widget webview) ---------------------

#[tauri::command]
fn widget_prefs(app: AppHandle) -> WidgetPrefs {
    windows::load_prefs(&app)
}

#[tauri::command]
fn widget_set_compact(app: AppHandle, compact: bool) {
    windows::set_compact(&app, compact);
}

#[tauri::command]
fn widget_set_opacity(app: AppHandle, opacity: f64) {
    windows::set_opacity(&app, opacity);
}

#[tauri::command]
fn widget_show(app: AppHandle) {
    windows::show(&app);
}

#[tauri::command]
fn widget_hide(app: AppHandle) {
    windows::hide(&app);
}

#[tauri::command]
fn widget_toggle(app: AppHandle) {
    windows::toggle(&app);
}

/// Enable/disable launch-on-login via the autostart plugin.
fn set_autostart(app: &AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    result.map_err(|e| format!("could not update launch-on-login: {e}"))
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
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(AppState {
            // Created with defaults; setup() loads the persisted config and
            // applies the real stale timeout before the listener starts.
            engine: Mutex::new(Engine::new(
                Duration::from_secs(config::DEFAULT_STALE_MIN * 60),
                Duration::from_secs(GRACE_SECS),
            )),
            config: Mutex::new(Config::default()),
            listener: Mutex::new(None),
            notifier: Notifier::new(),
        })
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            get_config,
            set_config,
            install_hooks,
            uninstall_hooks,
            hooks_installed,
            hook_block,
            endpoint,
            widget_prefs,
            widget_set_compact,
            widget_set_opacity,
            widget_show,
            widget_hide,
            widget_toggle
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            // Load persisted config and align runtime state with it.
            let cfg = config::load(&handle);
            {
                let state = handle.state::<AppState>();
                state
                    .engine
                    .lock()
                    .expect("engine mutex poisoned")
                    .set_stale_timeout(Duration::from_secs(cfg.stale_timeout_min * 60));
                *state.config.lock().expect("config mutex poisoned") = cfg.clone();
            }
            // Keep the OS autostart entry in sync with the saved preference.
            let _ = set_autostart(&handle, cfg.launch_on_login);

            // Tray-only app: no dock icon / app-switcher entry on macOS.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Build the tray (starts grey).
            tray::build(&handle)?;

            // Create the floating widget (shown only if it was visible last run).
            windows::init(&handle)?;

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

            // Start the localhost hook listener on the configured port.
            let server = spawn_listener(&handle, cfg.port).map_err(
                |e| -> Box<dyn std::error::Error> {
                    format!("failed to bind hook listener on 127.0.0.1:{}: {e}", cfg.port).into()
                },
            )?;
            *handle
                .state::<AppState>()
                .listener
                .lock()
                .expect("listener mutex poisoned") = Some(server);

            // Background stale sweep. Newly-stale sessions may notify if the
            // user enabled idle notifications.
            let sweep_handle = handle.clone();
            std::thread::Builder::new()
                .name("beacon-sweep".into())
                .spawn(move || loop {
                    std::thread::sleep(Duration::from_secs(SWEEP_INTERVAL_SECS));
                    let outcome = {
                        let state = sweep_handle.state::<AppState>();
                        let mut eng = state.engine.lock().expect("engine mutex poisoned");
                        eng.sweep()
                    };
                    if outcome.changed {
                        refresh(&sweep_handle);
                    }
                    // Idle isn't a tracked traffic-light State, so we notify
                    // directly (only when the user opted in) rather than routing
                    // through the per-state notifier.
                    if !outcome.went_stale.is_empty() {
                        let notify_idle_on = {
                            let state = sweep_handle.state::<AppState>();
                            let cfg = state.config.lock().expect("config mutex poisoned");
                            cfg.notify_idle
                        };
                        if notify_idle_on {
                            for (_id, label) in &outcome.went_stale {
                                notify_idle(&sweep_handle, label);
                            }
                        }
                    }
                })?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Beacon");
}

/// Fire an "went idle" notification (used only when `notify_idle` is enabled).
fn notify_idle(app: &AppHandle, label: &str) {
    use tauri_plugin_notification::NotificationExt;
    let _ = app
        .notification()
        .builder()
        .title("Beacon")
        .body(format!("{label} went idle"))
        .show();
}
