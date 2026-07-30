//! Session Signals — desktop status indicator for Claude Code.
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
mod glyph;
pub mod hooks;
pub mod ignore;
mod listener;
pub mod markers;
mod notify;
pub mod observe;
pub mod proposals;
pub mod token;
mod tray;
mod windows;

use config::Config;
use engine::{CapturedTerminal, Engine, HookEvent, Rollup, SessionView, Transition};
use ignore::{IgnoreRules, Matcher};
use notify::Notifier;
use observe::Observations;
use serde::Serialize;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_store::StoreExt;
use tiny_http::Server;
use tray::TrayPalette;
use windows::WidgetPrefs;

/// How often the background sweep checks for stale sessions.
const SWEEP_INTERVAL_SECS: u64 = 15;

/// `Mutex::lock` that recovers from poisoning instead of panicking. One
/// panicking lock-holder must not permanently poison state for every other
/// thread — a poisoned engine mutex would leave the app looking alive while
/// tracking nothing. Every critical section here is small and leaves the data
/// consistent, so continuing with the recovered guard is safe.
pub(crate) trait LockExt<T> {
    fn lock_safe(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> LockExt<T> for Mutex<T> {
    fn lock_safe(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Store file (shared with `windows.rs`) and the key under which captured
/// terminal handles are persisted: `{ session_id: { pid, app, tty } }`. They are
/// rehydrated at startup so click-to-focus survives a Session Signals restart even though
/// the capture hook only fires on `SessionStart`.
const STORE_FILE: &str = "beacon.json";
const KEY_CAPTURES: &str = "captures";
/// Key under which observed prompt-opening fingerprints are persisted
/// (`{fingerprint_hex: {len, n, first, last}}` — see `observe.rs`; never
/// prompt text).
const KEY_OBSERVATIONS: &str = "observations";

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
/// that are still running when Session Signals comes back up.
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
    let mut eng = state.engine.lock_safe();
    for (id, cap) in map {
        eng.seed_capture(id, cap);
    }
}

/// Load persisted observation records (fingerprint counts only), or an empty
/// store if absent/unreadable — never a panic.
fn load_observations(app: &AppHandle) -> Observations {
    let Ok(store) = app.store(STORE_FILE) else {
        return Observations::default();
    };
    match store.get(KEY_OBSERVATIONS) {
        Some(v) => Observations::from_json(v),
        None => Observations::default(),
    }
}

/// Flush observation records to the store. Best-effort: a save failure means
/// this run's counts are lost, not that the app stops (tolerated per plan —
/// counts only delay a future proposal, never block anything).
fn save_observations(app: &AppHandle, observations: &Observations) {
    let Ok(store) = app.store(STORE_FILE) else {
        return;
    };
    store.set(KEY_OBSERVATIONS, observations.to_json());
    let _ = store.save();
}

/// Current Unix time in seconds (wall clock — for `Observations::prune`,
/// which must survive a restart). `0` on an unreadable clock, which just
/// means nothing prunes this tick rather than panicking.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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
    /// Per-install secret salting observation fingerprints (see
    /// `observe::salt`). Empty until `setup()` loads/mints it — mirrors
    /// `token`, but under a separate store key so `regenerate_token` can
    /// never orphan observation history.
    observe_salt: Mutex<String>,
    /// Observed prompt-opening counts (salted-hash fingerprint → record;
    /// never plaintext). Flushed to the store on the sweep tick, not on every
    /// observation — see the sweep thread.
    observations: Mutex<Observations>,
    /// The suggestion count last pushed to the tray/webviews. `refresh_proposals`
    /// compares against this before doing any work — without it, a hot path
    /// (every `PostToolUse` heartbeat, across every session) would rebuild and
    /// swap the tray menu even when the count hasn't moved (review finding H1).
    suggestion_count: Mutex<usize>,
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
        let eng = state.engine.lock_safe();
        SessionsPayload {
            rollup: eng.rollup(),
            sessions: eng.snapshot(),
        }
    };
    {
        let palette = *state.tray_palette.lock_safe();
        tray::set_rollup(app, payload.rollup, &palette);
    }
    let _ = app.emit("sessions-updated", payload);
}

/// What the webview receives whenever the proposal count can have changed.
#[derive(Serialize, Clone)]
struct ProposalsPayload {
    count: usize,
}

/// Recompute the eligible-proposal count and, if it moved, push it to the
/// tray's quiet suggestion line and broadcast it to the webviews. Takes the
/// config lock then the observations lock (never nests the engine lock under
/// either) — so this must run on the `beacon-events` worker or the sweep
/// thread, never the listener thread.
///
/// Called from a hot path (`process_event`, on essentially every real hook
/// event across every tracked session), so the compare-and-set against
/// `suggestion_count` is load-bearing, not an optimization: without it, every
/// heartbeat would run `proposals::build` over the whole observation store
/// and unconditionally rebuild-and-swap the tray's native menu — the one
/// thing riskiest to do while the user might have that menu open (review
/// finding H1). The sweep tick's periodic call is the safety net that catches
/// any count change a hook event didn't (e.g. a dismissal lapsing with no
/// new event to hang the refresh off of).
fn refresh_proposals(app: &AppHandle) {
    let count = {
        let state = app.state::<AppState>();
        let cfg = state.config.lock_safe().clone();
        let ignore = IgnoreRules::new(cfg.ignore_rules.clone());
        let never_hide = IgnoreRules::new(cfg.never_hide.clone());
        let obs = state.observations.lock_safe();
        proposals::build(&obs, cfg.propose_threshold, &ignore, &never_hide).len()
    };
    {
        let state = app.state::<AppState>();
        let mut last = state.suggestion_count.lock_safe();
        if !suggestion_count_changed(&mut last, count) {
            return;
        }
    }
    tray::set_suggestion_count(app, count);
    let _ = app.emit("proposals-updated", ProposalsPayload { count });
}

/// The compare-and-set at the heart of `refresh_proposals`'s guard, pulled
/// out as a pure function so it's unit-testable without an `AppHandle` (this
/// crate has no Tauri test harness — see `proposals.rs`'s own doc comment for
/// the same constraint). Returns whether `count` differed from `*last`;
/// updates `*last` to `count` only when it did.
fn suggestion_count_changed(last: &mut usize, count: usize) -> bool {
    if *last == count {
        return false;
    }
    *last = count;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestion_count_unchanged_is_not_reported_as_changed() {
        let mut last = 3;
        assert!(
            !suggestion_count_changed(&mut last, 3),
            "same count → no change"
        );
        assert_eq!(last, 3, "unchanged value is left untouched");
    }

    #[test]
    fn suggestion_count_change_is_reported_and_stored() {
        let mut last = 0;
        assert!(suggestion_count_changed(&mut last, 2), "0 -> 2 is a change");
        assert_eq!(last, 2);
        assert!(
            !suggestion_count_changed(&mut last, 2),
            "calling again with the same count is a no-op"
        );
        assert!(
            suggestion_count_changed(&mut last, 0),
            "2 -> 0 (e.g. all proposals accepted/dismissed) is also a change"
        );
        assert_eq!(last, 0);
    }
}

/// React to a state transition: fire a notification per the user's config.
fn on_transition(app: &AppHandle, t: &Transition) {
    let state = app.state::<AppState>();
    let cfg = state.config.lock_safe().clone();
    state.notifier.fire(app, &cfg, t);
}

/// Apply one hook event and propagate its effects. Runs on the `beacon-events`
/// worker thread (never the listener thread), so heavy work — tray updates,
/// emits, OS notifications — can't delay ingestion of the next session's events.
fn process_event(app: &AppHandle, ev: HookEvent) {
    let outcome = {
        let state = app.state::<AppState>();
        let mut eng = state.engine.lock_safe();
        eng.apply(&ev)
    };
    // Persist (or forget) the terminal handle so click-to-focus survives a
    // Session Signals restart — the capture hook itself only fires on `SessionStart`.
    match ev.hook_event_name.as_str() {
        "BeaconTerminal" if ev.terminal_pid.is_some() => persist_capture(app, &ev),
        "SessionEnd" => forget_capture(app, &ev.session_id),
        _ => {}
    }
    // Derive the session descriptor from its transcript (debounced; bounded file
    // read done off the engine lock). A change is worth a UI refresh too.
    let desc_changed = maybe_refresh_descriptor(app, &ev);
    // First-prompt ignore rule (B): read the transcript head once, off-lock, and
    // hide the session if it opens with a known headless note. A newly-hidden
    // session must drop out of the widget/tray, so a change is worth a refresh.
    let hidden_changed = maybe_refresh_hidden(app, &ev);
    if outcome.changed || desc_changed || hidden_changed {
        refresh(app);
    }
    // A session's visibility or first-prompt classification changing is
    // exactly when the eligible-proposal set (and thus the tray's
    // suggestion count) can have moved too.
    if hidden_changed || outcome.changed {
        refresh_proposals(app);
    }
    if let Some(t) = outcome.transition {
        // Never notify for a filtered (headless/machine-spawned) session.
        let hidden = {
            let state = app.state::<AppState>();
            let eng = state.engine.lock_safe();
            eng.is_hidden(&t.session_id)
        };
        if !hidden {
            on_transition(app, &t);
        }
    }
}

/// How long to wait between transcript reads while a session still has no
/// descriptor (short, so it appears quickly) vs. once one is resolved (the
/// descriptor tracks the latest prompt, so keep this modest for freshness; a new
/// prompt also forces an immediate read — see below).
const DESCRIPTOR_RETRY_SECS: u64 = 5;
const DESCRIPTOR_REFRESH_SECS: u64 = 15;

/// How long before re-attempting a session's first-prompt head-read while it is
/// still unresolved. `SessionStart` carries a `transcript_path` but fires before
/// any prompt is written, so the first attempt reliably finds nothing; without a
/// retry the session could never be classified.
const FIRST_PROMPT_RETRY_SECS: u64 = 5;

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
            let eng = state.engine.lock_safe();
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
    let mut eng = state.engine.lock_safe();
    eng.set_descriptor(&ev.session_id, value)
}

/// First-prompt ignore rule (B) + observation ingest: when a `first_prompt_prefix`
/// matcher exists or observation is on, and the session isn't already hidden by
/// a (cheaper) cwd rule, read the transcript **head** once — off the engine
/// lock. The pipeline, in order (see the plan's Task 1 contract):
///   1. read → `FirstPrompt { text, human_marked }`
///   2. `observe_opening` — its own two guards (marker, then `never_hide`)
///      decide whether this opening is ever fingerprinted
///   3. `engine.set_first_prompt(text)` — **always last, unconditional**: it
///      feeds the user's own `ignore_rules`, independent of observation, so
///      skipping it for an allowlisted/marked session would be wrong.
///
/// Returns whether the session's hidden-ness changed (worth a UI refresh). A
/// cheap no-op in the common case: nothing to do, already checked, or a
/// synthetic event with no transcript.
fn maybe_refresh_hidden(app: &AppHandle, ev: &HookEvent) -> bool {
    let Some(path) = ev.transcript_path.as_deref() else {
        return false;
    };
    if ev.session_id.is_empty() {
        return false;
    }
    let state = app.state::<AppState>();
    // `UserPromptSubmit` fires as the prompt is *submitted* — Claude Code's
    // write of that turn to the transcript can still race the hook dispatch,
    // so a forced read here can legitimately come back empty. `Stop` fires
    // only after a full model turn has been produced, which is impossible
    // unless the user's prompt was already durably read from the transcript
    // — no race is possible by then, so forcing the read there too is
    // strictly safe, and it's what actually closes the gap: a fast,
    // tool-free round trip can lose the `UserPromptSubmit` race and, without
    // this, never get a second attempt before `SessionEnd` drops the
    // session (there's no `PreToolUse`/`PostToolUse` heartbeat to hang a
    // later retry off of, and `Stop` used to arrive inside the still-
    // elapsing retry window) — so its opening was never observed at all,
    // not merely filtered out downstream. `SessionEnd` itself is NOT
    // included: `Engine::apply` (called before this function, in
    // `process_event`) already removes the session from its map on
    // `SessionEnd`, so by the time `first_prompt_due` runs there, the
    // session is already gone and forcing here would be a no-op.
    // `Duration::ZERO` bypasses only the timer check inside
    // `first_prompt_due`; its other guards (observation/rules relevance,
    // already-resolved, cwd-hidden) still apply unchanged.
    let retry = if matches!(ev.hook_event_name.as_str(), "UserPromptSubmit" | "Stop") {
        Duration::ZERO
    } else {
        Duration::from_secs(FIRST_PROMPT_RETRY_SECS)
    };
    {
        let eng = state.engine.lock_safe();
        if !eng.first_prompt_due(&ev.session_id, retry) {
            return false;
        }
    }
    // Bounded head read — lock intentionally NOT held here.
    let fp = descriptor::first_prompt(path);
    if let Some(fp) = &fp {
        observe_opening(app, &ev.session_id, &ev.cwd, fp);
    }
    let mut eng = state.engine.lock_safe();
    eng.set_first_prompt(&ev.session_id, fp.map(|fp| fp.text))
}

/// The two guards from the ingest pipeline, run before anything is
/// fingerprinted: a human marker (built-in or config-added) means the
/// session's true opening was a person at the keyboard, and an opening the
/// user has allowlisted via `never_hide` must never touch disk at all — both
/// are checked before `Observations::observe` is called. Runs off the engine
/// lock (it's called from within the already-off-lock section of
/// `maybe_refresh_hidden`); takes the observations lock only for the
/// duration of `observe()`, and never while holding the engine lock, so the
/// two mutexes can't be acquired in opposing orders.
fn observe_opening(app: &AppHandle, session_id: &str, cwd: &str, fp: &descriptor::FirstPrompt) {
    let state = app.state::<AppState>();
    let cfg = state.config.lock_safe().clone();
    if !cfg.observe_enabled {
        return;
    }
    // A human marker preceded this prompt (slash command, IDE injection), or
    // the prompt itself opens with one a config-added marker declares Human
    // (a shape the transcript reader knows nothing about): the session's true
    // opening was a person at the keyboard. Never observed.
    let registry = markers::Registry::new(cfg.markers.clone());
    if fp.human_marked || registry.is_human(&fp.text) {
        return;
    }
    // The user has declared this opening their own. Allowlisted openings
    // never touch disk at all — strictly better than filtering at proposal
    // time, and no hash/plaintext comparison is ever needed.
    let never_hide = IgnoreRules::new(cfg.never_hide.clone());
    if never_hide.matches(cwd, Some(&fp.text)) {
        return;
    }
    let salt_hex = state.observe_salt.lock_safe().clone();
    let salt = observe::salt::bytes(&salt_hex);
    let mut observations = state.observations.lock_safe();
    observations.observe(&salt, session_id, &fp.text);
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
            let eng = state.engine.lock_safe();
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
    app.state::<AppState>().config.lock_safe().port
}

/// The current listener auth token.
fn current_token(app: &AppHandle) -> String {
    app.state::<AppState>().token.lock_safe().clone()
}

/// Install Session Signals' hooks for an explicit port + token, (re)writing the terminal
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
    let eng = state.engine.lock_safe();
    SessionsPayload {
        rollup: eng.rollup(),
        sessions: eng.snapshot(),
    }
}

#[tauri::command]
fn get_config(state: State<AppState>) -> Config {
    state.config.lock_safe().clone()
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
            let mut guard = state.listener.lock_safe();
            if let Some(old_server) = guard.take() {
                old_server.unblock();
            }
            *guard = Some(server);
        }
        // Keep settings.json pointed at the new port — but only if Session Signals'
        // hooks are actually installed (don't install just because port changed).
        if hooks::is_installed() {
            install_beacon_hooks_for(&app, new.port, &current_token(&app))?;
        }
    }

    // 4. Stale timeout + idle-drop window + session ignore rules.
    if new.stale_timeout_min != old.stale_timeout_min || new.idle_drop_min != old.idle_drop_min {
        let state = app.state::<AppState>();
        let mut eng = state.engine.lock_safe();
        eng.set_stale_timeout(Duration::from_secs(new.stale_timeout_min * 60));
        eng.set_drop_timeout(Duration::from_secs(new.idle_drop_min * 60));
    }
    if new.ignore_rules != old.ignore_rules {
        let state = app.state::<AppState>();
        let mut eng = state.engine.lock_safe();
        eng.set_ignore_rules(IgnoreRules::new(new.ignore_rules.clone()));
        drop(eng);
        // Re-judged sessions may drop out of (or back into) the widget/tray.
        refresh(&app);
    }
    // `never_hide` outranks `ignore_rules` — a newly-allowlisted opening
    // reveals a session immediately, same refresh-worthy reasoning as above.
    if new.never_hide != old.never_hide {
        let state = app.state::<AppState>();
        let mut eng = state.engine.lock_safe();
        eng.set_never_hide(IgnoreRules::new(new.never_hide.clone()));
        drop(eng);
        refresh(&app);
    }
    // Observation on/off takes effect immediately. Turning it off does NOT
    // clear existing records — that's an explicit act, not a side effect of
    // this toggle.
    if new.observe_enabled != old.observe_enabled {
        let state = app.state::<AppState>();
        let mut eng = state.engine.lock_safe();
        eng.set_observe_enabled(new.observe_enabled);
    }

    // 5. Persist + update live config.
    config::save(&app, &new)?;
    // Broadcast the new config so every window reacts — notably the theme: the
    // widget restyles even though the change was made in the settings window.
    let _ = app.emit("config-updated", &new);
    *app.state::<AppState>().config.lock_safe() = new;
    // A rule or threshold change can change which clusters are eligible
    // proposals — recompute the tray line every save, not just on rule
    // changes, since `propose_threshold`/`observe_enabled` also affect it.
    refresh_proposals(&app);
    Ok(())
}

/// Eligible filter proposals, highest count first, each with the live
/// sessions it would hide. Returns the full list (Phase 5's card renders
/// only the head — a list on screen invites bulk-accept, which is auto-hide
/// with extra steps — but the count drives the tray line).
#[tauri::command]
fn list_proposals(app: AppHandle) -> Vec<proposals::Proposal> {
    let state = app.state::<AppState>();
    let cfg = state.config.lock_safe().clone();
    let ignore = IgnoreRules::new(cfg.ignore_rules.clone());
    let never_hide = IgnoreRules::new(cfg.never_hide.clone());
    // Build under the observations lock, then drop it — the engine lock is
    // taken only after, mirroring `observe_opening`'s established order
    // (never nest the engine lock under the observations lock).
    let mut list = {
        let obs = state.observations.lock_safe();
        proposals::build(&obs, cfg.propose_threshold, &ignore, &never_hide)
    };
    let eng = state.engine.lock_safe();
    for p in &mut list {
        p.matching = eng.preview_hidden_by(Matcher::FirstPromptPrefix {
            value: p.sample.clone(),
        });
    }
    list
}

/// Accept a proposal: write its sample as a `first_prompt_prefix` ignore
/// rule via `set_config`, which owns persistence, the engine swap, and the
/// `config-updated` broadcast. Idempotent — accepting twice is a no-op, not
/// a duplicate rule.
#[tauri::command]
fn accept_proposal(app: AppHandle, fingerprint: String) -> Result<(), String> {
    let sample = {
        let state = app.state::<AppState>();
        let obs = state.observations.lock_safe();
        obs.sample_for(&fingerprint).map(|s| s.to_string())
    };
    let Some(sample) = sample else {
        return Err("proposal is no longer available".to_string());
    };
    let mut cfg = get_config(app.state::<AppState>());
    let matcher = Matcher::FirstPromptPrefix { value: sample };
    if cfg.ignore_rules.contains(&matcher) {
        return Ok(());
    }
    cfg.ignore_rules.push(matcher);
    set_config(app, cfg)
}

/// Refuse a proposal for this run only: recorded against its current cluster
/// count, so it reappears once the cluster grows past that count. Not
/// persisted — see `Observations::dismiss`.
#[tauri::command]
fn dismiss_proposal(app: AppHandle, fingerprint: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut obs = state.observations.lock_safe();
    let count = obs
        .iter_with_samples()
        .find(|(fp, _, _)| *fp == fingerprint)
        .map(|(_, rec, _)| rec.n);
    let Some(n) = count else {
        return Err("proposal is no longer available".to_string());
    };
    obs.dismiss(&fingerprint, n);
    Ok(())
}

/// Refuse a proposal permanently: add its sample to `never_hide` (via
/// `set_config`) and purge its fingerprint family from the observation
/// store. Purge runs **after** `set_config` succeeds — a failed config save
/// must not leave the cluster purged and the rule unwritten.
#[tauri::command]
fn never_suggest_proposal(app: AppHandle, fingerprint: String) -> Result<(), String> {
    let sample = {
        let state = app.state::<AppState>();
        let obs = state.observations.lock_safe();
        obs.sample_for(&fingerprint).map(|s| s.to_string())
    };
    let Some(sample) = sample else {
        return Err("proposal is no longer available".to_string());
    };
    let mut cfg = get_config(app.state::<AppState>());
    let matcher = Matcher::FirstPromptPrefix { value: sample };
    if !cfg.never_hide.contains(&matcher) {
        cfg.never_hide.push(matcher);
    }
    set_config(app.clone(), cfg)?;
    let state = app.state::<AppState>();
    let mut obs = state.observations.lock_safe();
    obs.purge_family(&fingerprint);
    save_observations(&app, &obs);
    Ok(())
}

/// Wipe every observation record immediately — the user clicked "clear" and
/// expects the store to change now, not on the next sweep tick.
#[tauri::command]
fn clear_observations(app: AppHandle) {
    let state = app.state::<AppState>();
    let mut obs = state.observations.lock_safe();
    obs.clear();
    save_observations(&app, &obs);
}

/// Everything the settings audit view needs in one call: how many sessions
/// are hidden and by which rule, how many times the reveal-on-block valve
/// has fired, and how many clusters are currently being observed (count
/// only — never sample text, matching the store's hash-only invariant).
#[derive(Serialize)]
struct FilterStatus {
    hidden_count: usize,
    /// How many times the reveal-on-block valve fired this run. Non-zero
    /// falsifies the "headless never blocks" premise — which is exactly why
    /// it is surfaced rather than logged.
    reveal_count: u64,
    hidden: Vec<engine::HiddenSession>,
    /// Live observation records. Count only — never sample text.
    observed_clusters: usize,
}

#[tauri::command]
fn filter_status(app: AppHandle) -> FilterStatus {
    let state = app.state::<AppState>();
    // Observations lock taken and dropped before the engine lock — mirrors
    // `list_proposals`'s established ordering (never nest engine under obs).
    let observed_clusters = {
        let obs = state.observations.lock_safe();
        obs.iter_with_samples().count()
    };
    let eng = state.engine.lock_safe();
    FilterStatus {
        hidden_count: eng.hidden_count(),
        reveal_count: eng.reveal_count(),
        hidden: eng.hidden_audit(),
        observed_clusters,
    }
}

/// The immutable built-in human markers, shipped from Rust so the UI cannot
/// drift from `markers::BUILTIN_HUMAN`.
#[tauri::command]
fn markers_builtin() -> Vec<String> {
    markers::BUILTIN_HUMAN
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// The measured proposal-eligibility floor, shipped from Rust so the settings
/// UI's short-entry warning (review finding M4) can never drift from the
/// constant `proposals::build` actually enforces.
#[tauri::command]
fn min_propose_sample_len() -> usize {
    config::MIN_PROPOSE_SAMPLE_LEN
}

/// Receive the active theme's palette from the webview, persist it, and restyle
/// the tray + notification icons. This is the *only* path appearance reaches the
/// native side, so a new theme is pure frontend data — no Rust change, no assets.
#[tauri::command]
fn set_tray_palette(app: AppHandle, palette: TrayPalette) {
    {
        let state = app.state::<AppState>();
        *state.tray_palette.lock_safe() = palette;
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
        *state.token.lock_safe() = fresh.clone();
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

/// Raise the terminal window that owns `session_id`, if Session Signals captured it.
/// Returns whether a window was resolved and a raise attempted — the widget uses
/// this to flash a "can't focus" hint on a false. Never errors/panics.
#[tauri::command]
fn focus_session(app: AppHandle, session_id: String) -> bool {
    let target = {
        let state = app.state::<AppState>();
        let eng = state.engine.lock_safe();
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
fn widget_set_expanded_height(app: AppHandle, height: f64) {
    windows::set_expanded_height(&app, height);
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
            // Same hardened path as the tray's "Open Session Signals…" so a relaunch can
            // never surface a stale/blank settings webview either.
            tray::show_settings(app);
        }))
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
            // Empty until setup() loads (or mints) the persisted salt —
            // mirrors `token` above, under its own store key.
            observe_salt: Mutex::new(String::new()),
            // Empty until setup() loads persisted observation records.
            observations: Mutex::new(Observations::default()),
            // 0 until the first `refresh_proposals` call (right after
            // `tray::build` in setup()) reconciles it with reality.
            suggestion_count: Mutex::new(0),
        })
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            get_config,
            set_config,
            list_proposals,
            accept_proposal,
            dismiss_proposal,
            never_suggest_proposal,
            clear_observations,
            filter_status,
            markers_builtin,
            min_propose_sample_len,
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
            widget_set_expanded_height,
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
                        // A panic while handling one event must not kill the
                        // worker: the listener would keep enqueueing into a dead
                        // channel and the app would look alive while tracking
                        // nothing. Drop the event, log, keep draining.
                        let h = &worker_handle;
                        let unwind =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                                process_event(h, ev)
                            }));
                        if unwind.is_err() {
                            eprintln!("beacon: panic while processing a hook event; event dropped");
                        }
                    }
                })?;

            // Load (or mint on first run) the listener auth token before the
            // listener binds, so it's enforcing a real secret from the start.
            let auth_token = token::load_or_create(&handle);
            *handle.state::<AppState>().token.lock_safe() = auth_token;

            // Load (or mint) the observation salt — a separate secret from
            // `auth_token` under its own store key (see `observe::salt`), and
            // load any observation records from a prior run.
            let observe_salt = observe::salt::load_or_create(&handle);
            *handle.state::<AppState>().observe_salt.lock_safe() = observe_salt;
            let observations = load_observations(&handle);
            *handle.state::<AppState>().observations.lock_safe() = observations;

            // Load persisted config and align runtime state with it.
            let cfg = config::load(&handle);
            // Load the persisted tray palette (classic until the webview pushes
            // the active theme) and render the matching notification icons.
            let palette = tray::load_palette(&handle);
            {
                let state = handle.state::<AppState>();
                let mut eng = state.engine.lock_safe();
                eng.set_stale_timeout(Duration::from_secs(cfg.stale_timeout_min * 60));
                eng.set_drop_timeout(Duration::from_secs(cfg.idle_drop_min * 60));
                eng.set_ignore_rules(IgnoreRules::new(cfg.ignore_rules.clone()));
                eng.set_never_hide(IgnoreRules::new(cfg.never_hide.clone()));
                eng.set_observe_enabled(cfg.observe_enabled);
                drop(eng);
                *state.config.lock_safe() = cfg.clone();
                *state.tray_palette.lock_safe() = palette;
            }
            notify::render_icons(&handle, &palette);
            // Keep the OS autostart entry in sync with the saved preference.
            let _ = set_autostart(&handle, cfg.launch_on_login);

            // Startup hook health: if Session Signals' hooks are installed but carry a
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
            // Reflect any proposals eligible from observation history loaded
            // above, so a restart doesn't silently drop the suggestion line
            // until the next qualifying hook event.
            refresh_proposals(&handle);

            // Create the floating widget (shown only if it was visible last run).
            windows::init(&handle)?;

            // Closing the settings window hides it rather than quitting Session Signals.
            if let Some(window) = app.get_webview_window("settings") {
                let w = window.clone();
                window.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = w.hide();
                    }
                });

                // First-run / not-set-up flow: if Session Signals' hooks aren't installed
                // yet, Session Signals can't detect anything — so surface the settings
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
            *handle.state::<AppState>().listener.lock_safe() = Some(server);

            // Background stale sweep. Newly-stale sessions may notify if the
            // user enabled idle notifications.
            let sweep_handle = handle.clone();
            std::thread::Builder::new()
                .name("beacon-sweep".into())
                .spawn(move || loop {
                    std::thread::sleep(Duration::from_secs(SWEEP_INTERVAL_SECS));
                    // Same rationale as the events worker: one panicking sweep
                    // must not end stale detection for the rest of the run.
                    let sweep_handle = &sweep_handle;
                    let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let outcome = {
                            let state = sweep_handle.state::<AppState>();
                            let mut eng = state.engine.lock_safe();
                            eng.sweep()
                        };
                        if outcome.changed {
                            refresh(sweep_handle);
                        }
                        // Idle isn't a tracked traffic-light State, so we notify
                        // directly (only when the user opted in) rather than routing
                        // through the per-state notifier.
                        if !outcome.went_stale.is_empty() {
                            let notify_idle_on = {
                                let state = sweep_handle.state::<AppState>();
                                let cfg = state.config.lock_safe();
                                cfg.notify_idle
                            };
                            if notify_idle_on {
                                for (_id, label) in &outcome.went_stale {
                                    notify_idle(sweep_handle, label);
                                }
                            }
                        }
                        // Prune expired observation records and flush to disk
                        // on this same heartbeat tick — piggybacking on the
                        // sweep rather than writing on every observation. A
                        // crash between ticks loses at most one interval's
                        // worth of counts, which only delays a proposal.
                        {
                            let retain_days = {
                                let state = sweep_handle.state::<AppState>();
                                let cfg = state.config.lock_safe();
                                cfg.observe_retain_days
                            };
                            let state = sweep_handle.state::<AppState>();
                            let mut observations = state.observations.lock_safe();
                            observations.prune(retain_days, now_secs());
                            if observations.take_dirty() {
                                save_observations(sweep_handle, &observations);
                            }
                        }
                        // Recompute the tray suggestion line every tick — a
                        // dismissal lapsing, a cluster crossing the
                        // threshold, or a pruned record can all move the
                        // count without any hook event to hang the refresh
                        // off of.
                        refresh_proposals(sweep_handle);
                    }));
                    if unwind.is_err() {
                        eprintln!("beacon: panic during stale sweep; skipping this pass");
                    }
                })?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Session Signals");
}

/// Fire an "went idle" notification (used only when `notify_idle` is enabled).
fn notify_idle(app: &AppHandle, label: &str) {
    use tauri_plugin_notification::NotificationExt;
    let _ = app
        .notification()
        .builder()
        .title("Session Signals")
        .body(format!("{label} went idle"))
        .show();
}
