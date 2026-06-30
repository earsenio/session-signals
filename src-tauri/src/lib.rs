//! Beacon — desktop status indicator for Claude Code.
//!
//! The Rust shell owns the source of truth (the state engine), the localhost
//! hook listener, the tray, and now configuration + notifications. The webviews
//! (settings + widget) are thin renderers: they ask for snapshots/config and
//! listen for updates. State flows one way — engine → events → UI.

pub mod capture;
pub mod config;
pub mod descriptor;
pub mod engine;
pub mod focus;
pub mod hooks;
mod listener;
mod notify;
pub mod token;
mod tray;
mod windows;

use config::Config;
use engine::{CapturedTerminal, Engine, HookEvent, Rollup, SessionView, Transition};
use notify::Notifier;
use serde::Serialize;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_store::StoreExt;
use tiny_http::Server;
use tray::TrayPalette;
use windows::WidgetPrefs;

/// How often the background sweep checks for stale sessions.
const SWEEP_INTERVAL_SECS: u64 = 15;

/// Store file (shared with `windows.rs`) and the key under which captured
/// terminal handles are persisted: `{ session_id: { pid, app, tty } }`. They are
/// rehydrated at startup so click-to-focus survives a Beacon restart even though
/// the capture hook only fires on `SessionStart`.
const STORE_FILE: &str = "beacon.json";
const KEY_CAPTURES: &str = "captures";

/// Persist a freshly-captured terminal handle, keyed by session id. Best-effort:
/// a store error just means this session won't survive a restart for focus.
fn persist_capture(app: &AppHandle, ev: &HookEvent) {
    let Ok(store) = app.store(STORE_FILE) else {
        return;
    };
    let mut map = store
        .get(KEY_CAPTURES)
        .and_then(|v| {
            serde_json::from_value::<std::collections::HashMap<String, CapturedTerminal>>(v).ok()
        })
        .unwrap_or_default();
    map.insert(
        ev.session_id.clone(),
        CapturedTerminal {
            pid: ev.terminal_pid,
            app: ev.terminal_app.clone(),
            tty: ev.terminal_tty.clone(),
        },
    );
    if let Ok(v) = serde_json::to_value(&map) {
        store.set(KEY_CAPTURES, v);
        let _ = store.save();
    }
}

/// Drop a session's persisted handle (on `SessionEnd`) so the store doesn't
/// accumulate handles for terminals that no longer exist.
fn forget_capture(app: &AppHandle, session_id: &str) {
    let Ok(store) = app.store(STORE_FILE) else {
        return;
    };
    let Some(mut map) = store.get(KEY_CAPTURES).and_then(|v| {
        serde_json::from_value::<std::collections::HashMap<String, CapturedTerminal>>(v).ok()
    }) else {
        return;
    };
    if map.remove(session_id).is_some() {
        if let Ok(v) = serde_json::to_value(&map) {
            store.set(KEY_CAPTURES, v);
            let _ = store.save();
        }
    }
}

/// Seed remembered terminal handles into the engine at startup. They attach to a
/// session only when a real hook event recreates its row, so this can never
/// resurrect a phantom session — it just restores click-to-focus for sessions
/// that are still running when Beacon comes back up.
fn seed_captures(app: &AppHandle) {
    let Ok(store) = app.store(STORE_FILE) else {
        return;
    };
    let Some(map) = store.get(KEY_CAPTURES).and_then(|v| {
        serde_json::from_value::<std::collections::HashMap<String, CapturedTerminal>>(v).ok()
    }) else {
        return;
    };
    let state = app.state::<AppState>();
    let mut eng = state.engine.lock().expect("engine mutex poisoned");
    for (id, cap) in map {
        eng.seed_capture(id, cap);
    }
}

/// Shared application state, managed by Tauri.
struct AppState {
    engine: Mutex<Engine>,
    config: Mutex<Config>,
    /// The currently-bound hook listener. Held so a port change can stop it
    /// (`unblock`) and swap in a new one.
    listener: Mutex<Option<Arc<Server>>>,
    /// Shared listener auth token. Every installed hook posts it as a header;
    /// the listener checks it on each request. Held in an `Arc<Mutex>` so a
    /// "regenerate" swaps the secret live without restarting the listener.
    token: listener::AuthToken,
    /// The active theme's tray palette, pushed from the webview. The tray icon is
    /// drawn from this; persisted so the look survives restarts.
    tray_palette: Mutex<TrayPalette>,
    notifier: Notifier,
    /// Hook events flow listener → this channel → the `beacon-events` worker.
    /// Keeping the listener thread off the engine/notify work means one
    /// session's processing can never stall another's ingestion.
    events: Sender<HookEvent>,
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
    {
        let palette = *state.tray_palette.lock().expect("palette mutex poisoned");
        tray::set_rollup(app, payload.rollup, &palette);
    }
    let _ = app.emit("sessions-updated", payload);
}

/// React to a state transition: fire a notification per the user's config.
fn on_transition(app: &AppHandle, t: &Transition) {
    let state = app.state::<AppState>();
    let cfg = state.config.lock().expect("config mutex poisoned").clone();
    state.notifier.fire(app, &cfg, t);
}

/// Apply one hook event and propagate its effects. Runs on the `beacon-events`
/// worker thread (never the listener thread), so heavy work — tray updates,
/// emits, OS notifications — can't delay ingestion of the next session's events.
fn process_event(app: &AppHandle, ev: HookEvent) {
    let outcome = {
        let state = app.state::<AppState>();
        let mut eng = state.engine.lock().expect("engine mutex poisoned");
        eng.apply(&ev)
    };
    // Persist (or forget) the terminal handle so click-to-focus survives a
    // Beacon restart — the capture hook itself only fires on `SessionStart`.
    match ev.hook_event_name.as_str() {
        "BeaconTerminal" if ev.terminal_pid.is_some() => persist_capture(app, &ev),
        "SessionEnd" => forget_capture(app, &ev.session_id),
        _ => {}
    }
    // Derive the session descriptor from its transcript (debounced; bounded file
    // read done off the engine lock). A change is worth a UI refresh too.
    let desc_changed = maybe_refresh_descriptor(app, &ev);
    if outcome.changed || desc_changed {
        refresh(app);
    }
    if let Some(t) = outcome.transition {
        on_transition(app, &t);
    }
}

/// How long to wait between transcript reads while a session still has no
/// descriptor (short, so it appears quickly) vs. once one is resolved (the
/// descriptor tracks the latest prompt, so keep this modest for freshness; a new
/// prompt also forces an immediate read — see below).
const DESCRIPTOR_RETRY_SECS: u64 = 5;
const DESCRIPTOR_REFRESH_SECS: u64 = 15;

/// Derive/refresh a session's descriptor from its transcript. Debounced via the
/// engine (`descriptor_due`); the bounded file read runs with the engine lock
/// released so transcript I/O never blocks other sessions' event processing.
/// Returns whether the displayed descriptor changed.
fn maybe_refresh_descriptor(app: &AppHandle, ev: &HookEvent) -> bool {
    // Only real Claude hook events carry a transcript; the synthetic
    // `BeaconTerminal` does not.
    let Some(path) = ev.transcript_path.as_deref() else {
        return false;
    };
    if ev.session_id.is_empty() {
        return false;
    }
    let state = app.state::<AppState>();
    // A freshly-submitted prompt is exactly when the descriptor changes, so read
    // it right away instead of waiting out the debounce.
    let force = ev.hook_event_name == "UserPromptSubmit";
    if !force {
        let due = {
            let eng = state.engine.lock().expect("engine mutex poisoned");
            eng.descriptor_due(
                &ev.session_id,
                Duration::from_secs(DESCRIPTOR_RETRY_SECS),
                Duration::from_secs(DESCRIPTOR_REFRESH_SECS),
            )
        };
        if !due {
            return false;
        }
    }
    // Bounded transcript read — lock intentionally NOT held here.
    let value = descriptor::extract(path);
    let mut eng = state.engine.lock().expect("engine mutex poisoned");
    eng.set_descriptor(&ev.session_id, value)
}

/// Build and start a listener on `port`. The hook callback does the minimum —
/// hand the event to the worker channel and return — so the listener thread is
/// always free to accept the next request. Returns the server handle (or a bind
/// error). The `/state` readback closure reports the current rollup + snapshot.
fn spawn_listener(app: &AppHandle, port: u16) -> std::io::Result<Arc<Server>> {
    let tx = app.state::<AppState>().events.clone();
    let auth = app.state::<AppState>().token.clone();
    let state_handle = app.clone();
    listener::start(
        port,
        auth,
        move |ev| {
            // Non-blocking: just enqueue. Ordering is preserved (single sender
            // per listener, single receiver).
            let _ = tx.send(ev);
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

/// The current listener auth token.
fn current_token(app: &AppHandle) -> String {
    app.state::<AppState>()
        .token
        .lock()
        .expect("token mutex poisoned")
        .clone()
}

/// Install Beacon's hooks for an explicit port + token, (re)writing the terminal
/// capture script first so the `SessionStart` command hook targets that listener.
/// Capture is best-effort: if the script can't be written, the http hooks still
/// install (click-to-focus simply won't be available). Takes the port/token
/// explicitly because callers (e.g. a port change) need to install for the *new*
/// values before the live config has been committed.
fn install_beacon_hooks_for(
    app: &AppHandle,
    port: u16,
    token: &str,
) -> Result<std::path::PathBuf, String> {
    let capture_cmd = capture::write_script(app, port, token);
    hooks::install(port, token, capture_cmd.as_deref())
}

/// Install for the currently-live port + token.
fn install_beacon_hooks(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    install_beacon_hooks_for(app, current_port(app), &current_token(app))
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
            install_beacon_hooks_for(&app, new.port, &current_token(&app))?;
        }
    }

    // 4. Stale timeout + idle-drop window.
    if new.stale_timeout_min != old.stale_timeout_min || new.idle_drop_min != old.idle_drop_min {
        let state = app.state::<AppState>();
        let mut eng = state.engine.lock().expect("engine mutex poisoned");
        eng.set_stale_timeout(Duration::from_secs(new.stale_timeout_min * 60));
        eng.set_drop_timeout(Duration::from_secs(new.idle_drop_min * 60));
    }

    // 5. Persist + update live config.
    config::save(&app, &new)?;
    // Broadcast the new config so every window reacts — notably the theme: the
    // widget restyles even though the change was made in the settings window.
    let _ = app.emit("config-updated", &new);
    *app.state::<AppState>()
        .config
        .lock()
        .expect("config mutex poisoned") = new;
    Ok(())
}

/// Receive the active theme's palette from the webview, persist it, and restyle
/// the tray + notification icons. This is the *only* path appearance reaches the
/// native side, so a new theme is pure frontend data — no Rust change, no assets.
#[tauri::command]
fn set_tray_palette(app: AppHandle, palette: TrayPalette) {
    {
        let state = app.state::<AppState>();
        *state.tray_palette.lock().expect("palette mutex poisoned") = palette;
    }
    tray::save_palette(&app, &palette);
    notify::render_icons(&app, &palette);
    // Repaint the tray with the current rollup in the new palette.
    refresh(&app);
}

#[tauri::command]
fn install_hooks(app: AppHandle) -> Result<String, String> {
    install_beacon_hooks(&app).map(|p| p.display().to_string())
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
    hooks::hook_block_string(current_port(&app), &current_token(&app))
}

/// Mint a fresh listener token, swap it into the live listener, and — if hooks
/// are installed — rewrite settings.json so the hooks carry the new token. The
/// listener reads the shared token on each request, so sessions keep flowing
/// across the swap. Order: persist first, then update the live value, so a save
/// failure leaves the running token (and settings.json) untouched.
#[tauri::command]
fn regenerate_token(app: AppHandle) -> Result<(), String> {
    let fresh = token::regenerate(&app)?;
    {
        let state = app.state::<AppState>();
        *state.token.lock().expect("token mutex poisoned") = fresh.clone();
    }
    // Re-run the installer so the hooks' header (and capture script) match the
    // new token. Only if they're actually installed.
    if hooks::is_installed() {
        install_beacon_hooks_for(&app, current_port(&app), &fresh)?;
    }
    Ok(())
}

#[tauri::command]
fn endpoint(app: AppHandle) -> String {
    hooks::endpoint(current_port(&app))
}

/// Raise the terminal window that owns `session_id`, if Beacon captured it.
/// Returns whether a window was resolved and a raise attempted — the widget uses
/// this to flash a "can't focus" hint on a false. Never errors/panics.
#[tauri::command]
fn focus_session(app: AppHandle, session_id: String) -> bool {
    let target = {
        let state = app.state::<AppState>();
        let eng = state.engine.lock().expect("engine mutex poisoned");
        eng.focus_target(&session_id)
    };
    match target {
        Some((pid, tty, app_name)) => focus::raise(&focus::FocusTarget {
            pid,
            tty,
            app: app_name,
        }),
        None => false,
    }
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
fn widget_set_compact_width(app: AppHandle, width: f64) {
    windows::set_compact_width(&app, width);
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
    // Hook events flow: listener thread → this channel → the `beacon-events`
    // worker (spawned in setup, owns `rx`). Created here so the sender can live
    // in AppState and the receiver can move into the setup closure.
    let (tx, rx) = std::sync::mpsc::channel::<HookEvent>();

    tauri::Builder::default()
        // Single-instance must be registered first: a second launch just
        // surfaces the existing settings window instead of fighting over the
        // listener port.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // Same hardened path as the tray's "Open Beacon…" so a relaunch can
            // never surface a stale/blank settings webview either.
            tray::show_settings(app);
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
                Duration::from_secs(config::DEFAULT_IDLE_DROP_MIN * 60),
            )),
            config: Mutex::new(Config::default()),
            listener: Mutex::new(None),
            // Empty until setup() loads (or mints) the persisted token. The
            // listener fails closed on an empty token, so nothing is accepted
            // before setup runs.
            token: Arc::new(Mutex::new(String::new())),
            // Classic until setup() loads the persisted palette.
            tray_palette: Mutex::new(TrayPalette::default()),
            notifier: Notifier::new(),
            events: tx,
        })
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            get_config,
            set_config,
            set_tray_palette,
            install_hooks,
            uninstall_hooks,
            hooks_installed,
            hook_block,
            regenerate_token,
            endpoint,
            focus_session,
            widget_prefs,
            widget_set_compact,
            widget_set_compact_width,
            widget_set_opacity,
            widget_show,
            widget_hide,
            widget_toggle
        ])
        .setup(move |app| {
            let handle = app.handle().clone();

            // Drain hook events on a dedicated worker thread, in receive order.
            // The listener only enqueues; all engine/refresh/notify work happens
            // here, so one session can never block another's ingestion.
            let worker_handle = handle.clone();
            std::thread::Builder::new()
                .name("beacon-events".into())
                .spawn(move || {
                    for ev in rx {
                        process_event(&worker_handle, ev);
                    }
                })?;

            // Load (or mint on first run) the listener auth token before the
            // listener binds, so it's enforcing a real secret from the start.
            let auth_token = token::load_or_create(&handle);
            *handle
                .state::<AppState>()
                .token
                .lock()
                .expect("token mutex poisoned") = auth_token;

            // Load persisted config and align runtime state with it.
            let cfg = config::load(&handle);
            // Load the persisted tray palette (classic until the webview pushes
            // the active theme) and render the matching notification icons.
            let palette = tray::load_palette(&handle);
            {
                let state = handle.state::<AppState>();
                let mut eng = state.engine.lock().expect("engine mutex poisoned");
                eng.set_stale_timeout(Duration::from_secs(cfg.stale_timeout_min * 60));
                eng.set_drop_timeout(Duration::from_secs(cfg.idle_drop_min * 60));
                drop(eng);
                *state.config.lock().expect("config mutex poisoned") = cfg.clone();
                *state.tray_palette.lock().expect("palette mutex poisoned") = palette;
            }
            notify::render_icons(&handle, &palette);
            // Keep the OS autostart entry in sync with the saved preference.
            let _ = set_autostart(&handle, cfg.launch_on_login);

            // Startup hook health: if Beacon's hooks are installed but carry a
            // stale/absent auth-token header (e.g. after upgrading to a
            // token-enforcing build over pre-token hooks), the listener would
            // 401 every event and silently track nothing. Auto-repair by
            // re-running the installer with the live port + token — this also
            // refreshes the capture script. The not-installed case is left to
            // the first-run flow below.
            if hooks::is_installed() && hooks::needs_token_repair(&current_token(&handle)) {
                match install_beacon_hooks(&handle) {
                    Ok(p) => eprintln!("beacon: repaired stale hook auth token in {}", p.display()),
                    Err(e) => eprintln!("beacon: could not repair stale hooks: {e}"),
                }
            }

            // Tray-only app: no dock icon / app-switcher entry on macOS.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Build the tray (starts grey) using the persisted palette.
            tray::build(&handle, &palette)?;

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

                // First-run / not-set-up flow: if Beacon's hooks aren't installed
                // yet, Beacon can't detect anything — so surface the settings
                // window (which shows the install banner) instead of sitting as a
                // silent grey tray icon the user can't act on.
                if !hooks::is_installed() {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }

            // Rehydrate terminal handles captured before the last shutdown, so
            // click-to-focus works for still-running sessions the moment they
            // next emit any hook — without waiting for a fresh `SessionStart`.
            // Seeded before the listener binds, so it's in place for the first
            // event that recreates a session row.
            seed_captures(&handle);

            // Start the localhost hook listener on the configured port.
            let server =
                spawn_listener(&handle, cfg.port).map_err(|e| -> Box<dyn std::error::Error> {
                    format!(
                        "failed to bind hook listener on 127.0.0.1:{}: {e}",
                        cfg.port
                    )
                    .into()
                })?;
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
