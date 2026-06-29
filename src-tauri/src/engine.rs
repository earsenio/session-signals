//! State engine: the single source of truth for session status.
//!
//! Sessions are keyed by `session_id`. Hook events mutate per-session state
//! following the derivation rules in CLAUDE.md. The engine also computes the
//! tray rollup and sweeps stale (silent) sessions. It holds no Tauri handles —
//! `lib.rs` owns it behind a `Mutex` and reacts to changes by refreshing the
//! tray and emitting to the webview. The UI never derives state itself.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Per-session status. Maps to the traffic-light colors in the spec.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// 🔴 Blocked on the user (permission / choice / answer).
    NeedsYou,
    /// 🟠 Actively running.
    Working,
    /// 🟢 Finished its turn — okay to give new instructions.
    Ready,
}

/// The tray rollup across all live sessions.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Rollup {
    Red,
    Orange,
    Green,
    Grey,
}

/// A single tracked Claude Code session.
#[derive(Clone, Debug)]
struct Session {
    cwd: String,
    state: State,
    /// Last time we heard anything from this session.
    last_seen: Instant,
    /// When the current `state` was entered (for time-in-state display).
    state_since: Instant,
    /// True once it has gone silent past the stale timeout (display grey,
    /// excluded from rollup) but before the grace drop.
    stale: bool,
    /// Live subagents fanned out from this session: `SubagentStart` minus
    /// `SubagentStop`, clamped at ≥ 0. Drives the row's "N agents running"
    /// sub-line independently of `state`.
    subagent_count: u32,
    /// When `subagent_count` last rose from 0 — the anchor for the sub-line's
    /// ticking elapsed timer. `None` whenever the count is 0.
    sub_since: Option<Instant>,
    /// PID of the terminal *application* hosting this session, captured by the
    /// `SessionStart` command hook (see `capture.rs`). Drives click-to-focus and
    /// focus-aware notifications. `None` until/unless capture resolves it.
    terminal_pid: Option<i32>,
    /// Human name of that terminal app (e.g. "iTerm2", "WindowsTerminal.exe"),
    /// for display/debugging. Best-effort.
    terminal_app: Option<String>,
    /// Controlling tty of this session (e.g. "/dev/ttys003"), captured by walking
    /// the parent chain. Lets `focus.rs` select the exact tab/window on macOS
    /// terminals that expose a per-tab tty (Terminal.app, iTerm2), rather than
    /// only raising the app. `None` on Windows / when unresolved.
    terminal_tty: Option<String>,
}

/// Parsed, transport-agnostic hook event. The listener deserializes the raw
/// JSON into this; the engine never sees HTTP.
#[derive(Debug, serde::Deserialize, Default)]
pub struct HookEvent {
    pub hook_event_name: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub cwd: String,
    /// Present only on `Notification` events.
    #[serde(default)]
    pub notification_type: Option<String>,
    /// Identifies the agent that emitted the event. **Present (non-null) only on
    /// subagent events; null/absent on the main agent.** A subagent shares its
    /// parent's `session_id`, so this is the only signal that an event came from
    /// a fanned-out subagent rather than the main agent. Verified empirically on
    /// Claude Code 2.1.x (subagent `PreToolUse`/`PostToolUse`/`SubagentStart`/
    /// `SubagentStop` all carry it; main-agent events do not). Used to keep
    /// subagent activity from overwriting the session's traffic-light state.
    #[serde(default)]
    pub agent_id: Option<String>,
    /// The subagent's type (e.g. "Explore"), present alongside `agent_id`.
    /// Captured for display/debugging; not currently used for state logic.
    #[serde(default)]
    pub agent_type: Option<String>,
    /// Present only on the synthetic `BeaconTerminal` event from the capture
    /// hook: the owning terminal app's pid and name.
    #[serde(default)]
    pub terminal_pid: Option<i32>,
    #[serde(default)]
    pub terminal_app: Option<String>,
    /// Present only on `BeaconTerminal`: the session's controlling tty.
    #[serde(default)]
    pub terminal_tty: Option<String>,
}

/// A state change for one session, reported by `apply` so the notification
/// engine can react. `from` is `None` when the session is brand new.
#[derive(Serialize, Clone, Debug)]
pub struct Transition {
    pub session_id: String,
    pub label: String,
    pub from: Option<State>,
    pub to: State,
    /// The session's captured terminal pid at transition time, if known. Lets
    /// the notifier suppress alerts when that terminal is already frontmost.
    pub terminal_pid: Option<i32>,
}

/// Result of applying one hook event.
pub struct ApplyOutcome {
    /// True if the rollup / session list may have changed (worth a UI refresh).
    pub changed: bool,
    /// Present only when a session actually moved to a new state.
    pub transition: Option<Transition>,
}

/// Result of a stale sweep.
pub struct SweepOutcome {
    pub changed: bool,
    /// Sessions that newly went stale this sweep: `(session_id, label)`.
    pub went_stale: Vec<(String, String)>,
}

/// A flattened, serializable view of one session for the webview / tray menu.
#[derive(Serialize, Clone, Debug)]
pub struct SessionView {
    pub session_id: String,
    pub label: String,
    pub state: State,
    pub stale: bool,
    /// Seconds the session has been in its current state.
    pub seconds_in_state: u64,
    /// Live subagents running under this session (`SubagentStart` − `SubagentStop`).
    pub subagent_count: u32,
    /// Seconds since the subagent count rose from 0 (0 when none are running).
    pub subagent_seconds: u64,
    /// Whether Beacon resolved the owning terminal window — gates the widget's
    /// click-to-focus affordance (no handle ⇒ no focus button).
    pub can_focus: bool,
}

/// A terminal handle remembered across a Beacon restart. Capture lives only in
/// memory and only fires on `SessionStart` (see `capture.rs`), so a restart
/// would otherwise lose click-to-focus for every already-running session until
/// it happens to start a new turn. `lib.rs` persists these to the store and
/// seeds them back in here at startup; they are a *side table* — they attach to
/// a session only when a real hook event (re)creates its row, and never conjure
/// a row on their own.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CapturedTerminal {
    pub pid: Option<i32>,
    pub app: Option<String>,
    pub tty: Option<String>,
}

pub struct Engine {
    sessions: HashMap<String, Session>,
    /// Remembered terminal handles keyed by `session_id`, rehydrated at startup.
    /// Consulted when a session is (re)created so a restart keeps click-to-focus;
    /// never iterated to build the session list (it cannot create rows).
    pending_captures: HashMap<String, CapturedTerminal>,
    stale_timeout: Duration,
    /// Total silence before a session is removed from the list entirely. An
    /// idle session is visibly greyed ("No response") for the whole window
    /// between `stale_timeout` and this; only then is it dropped. Removal is
    /// otherwise driven by an explicit `SessionEnd`. Kept long (config default
    /// 60 min) so an idle session persists rather than blinking out, while a
    /// terminal killed without firing `SessionEnd` still eventually self-clears.
    drop_timeout: Duration,
}

impl Engine {
    pub fn new(stale_timeout: Duration, drop_timeout: Duration) -> Self {
        Engine {
            sessions: HashMap::new(),
            pending_captures: HashMap::new(),
            stale_timeout,
            drop_timeout,
        }
    }

    /// Apply a hook event. Reports whether a UI refresh is worthwhile and, if a
    /// session actually changed state, the transition (for notifications).
    pub fn apply(&mut self, ev: &HookEvent) -> ApplyOutcome {
        // An empty session_id is unusable as a key; ignore but don't crash.
        if ev.session_id.is_empty() && ev.hook_event_name != "SessionEnd" {
            return ApplyOutcome {
                changed: false,
                transition: None,
            };
        }

        // A subagent (Task tool) shares its parent's `session_id`, so without
        // this its events would mutate the parent row's traffic-light state — a
        // running subagent could clear a real "Needs you" (verified: subagent
        // events carry `agent_id`, the main agent's do not). The rule: only the
        // MAIN agent moves a session into Working/Ready; subagent events are
        // heartbeat-only (they keep the session live + drive the subagent count,
        // but never change its color). A genuine *block* is the one exception —
        // see `Notification` below — because a subagent hitting a permission gate
        // still needs the user, so it must be allowed to escalate to NeedsYou.
        let is_subagent = ev.agent_id.is_some();

        match ev.hook_event_name.as_str() {
            // A fresh (or resumed) session: clear any leftover subagent count so a
            // restart never inherits a stale "N agents running" sub-line. (Main
            // agent only; a subagent never legitimately starts a session — if one
            // somehow does, treat it as a heartbeat, not a reset.)
            "SessionStart" => {
                if is_subagent {
                    self.heartbeat(ev)
                } else {
                    let out = self.transition_to(ev, State::Ready);
                    self.reset_subagents(&ev.session_id);
                    out
                }
            }
            // Any work-start signal means the session is actively running. We
            // bracket "Working" between these and the terminal events below, so
            // activity that doesn't begin with a typed prompt — slash-command
            // expansion, a tool call, context compaction — still shows up.
            // (`/compact` fires PreCompact, never UserPromptSubmit, which is why
            // it used to stay green.) A subagent's tool call must NOT flip the
            // parent's color, so it heartbeats instead.
            "UserPromptSubmit" | "UserPromptExpansion" | "PreToolUse" | "PreCompact" => {
                if is_subagent {
                    self.heartbeat(ev)
                } else {
                    self.transition_to(ev, State::Working)
                }
            }
            // A spawned subagent: bump the live subagent count (the first one
            // anchors the sub-line's elapsed timer). This does NOT change the
            // session's state — the main agent's own `PreToolUse` for the Task
            // tool already moved it to Working; the count drives the independent
            // "N agents running" sub-line.
            "SubagentStart" => {
                let out = self.heartbeat(ev);
                if let Some(s) = self.sessions.get_mut(&ev.session_id) {
                    if s.subagent_count == 0 {
                        s.sub_since = Some(Instant::now());
                    }
                    s.subagent_count += 1;
                }
                out
            }
            // Heartbeat: keep current state, just refresh last_seen. Also the
            // landing spot for any main-agent work-start / terminal event that
            // arrived from a subagent (`is_subagent`) — those keep the session
            // live without recoloring it.
            "PostToolUse" | "PostToolUseFailure" | "PostToolBatch" => self.heartbeat(ev),
            "Notification" => match ev.notification_type.as_deref() {
                // Only a genuine block on the user is "Needs you". This is the one
                // state a subagent IS allowed to set: if a fanned-out subagent hits
                // a permission gate, the user still has to act, so we escalate to
                // NeedsYou regardless of `is_subagent`.
                Some("permission_prompt") | Some("elicitation_dialog") => {
                    self.transition_to(ev, State::NeedsYou)
                }
                // `idle_prompt` fires when a session has merely been sitting
                // idle — it is NOT blocked on the user. Leave its state alone
                // (a finished turn stays Ready/green; a pending permission stays
                // red); the stale sweep greys it out after the timeout.
                // auth_success, elicitation_complete, etc. are likewise ignored.
                _ => ApplyOutcome {
                    changed: false,
                    transition: None,
                },
            },
            // Terminal: the turn (or compaction) ended. `PostCompact` returns a
            // standalone `/compact` to Ready; mid-turn it briefly shows Ready
            // until the next work event flips it back (self-healing).
            // `StopFailure` is a turn ended by an API error. Only the MAIN agent's
            // turn ending means the row is Ready — a subagent's `Stop` must not
            // clear the parent's state (esp. a pending "Needs you").
            "Stop" | "StopFailure" | "PostCompact" => {
                if is_subagent {
                    self.heartbeat(ev)
                } else {
                    self.transition_to(ev, State::Ready)
                }
            }
            // A subagent finished: decrement (clamped), and when the last one
            // leaves, drop the elapsed anchor so the sub-line disappears. Like
            // `SubagentStart`, this only touches the count — it does NOT move the
            // session to Ready, which previously flipped a still-working (or
            // still-blocked) parent to green the instant any subagent stopped.
            "SubagentStop" => {
                let out = self.heartbeat(ev);
                if let Some(s) = self.sessions.get_mut(&ev.session_id) {
                    s.subagent_count = s.subagent_count.saturating_sub(1);
                    if s.subagent_count == 0 {
                        s.sub_since = None;
                    }
                }
                out
            }
            // Synthetic event from the terminal-capture hook: record which
            // terminal owns this session. No state change — a session can be in
            // any color and still get (or refresh) its terminal mapping. Creates
            // the session if it raced ahead of SessionStart so the pid isn't lost.
            "BeaconTerminal" => {
                let now = Instant::now();
                let s = self
                    .sessions
                    .entry(ev.session_id.clone())
                    .or_insert_with(|| Session {
                        cwd: ev.cwd.clone(),
                        state: State::Ready,
                        last_seen: now,
                        state_since: now,
                        stale: false,
                        subagent_count: 0,
                        sub_since: None,
                        terminal_pid: None,
                        terminal_app: None,
                        terminal_tty: None,
                    });
                if ev.terminal_pid.is_some() {
                    s.terminal_pid = ev.terminal_pid;
                }
                if ev.terminal_app.is_some() {
                    s.terminal_app = ev.terminal_app.clone();
                }
                if ev.terminal_tty.as_deref().is_some_and(|t| !t.is_empty()) {
                    s.terminal_tty = ev.terminal_tty.clone();
                }
                s.last_seen = now;
                if !ev.cwd.is_empty() {
                    s.cwd = ev.cwd.clone();
                }
                ApplyOutcome {
                    changed: true,
                    transition: None,
                }
            }
            "SessionEnd" => {
                // The terminal is gone; forget its remembered handle too so a
                // stale entry can't outlive the session in the persisted store.
                self.pending_captures.remove(&ev.session_id);
                ApplyOutcome {
                    changed: self.sessions.remove(&ev.session_id).is_some(),
                    transition: None,
                }
            }
            // Unknown / unhandled event: ignore.
            _ => ApplyOutcome {
                changed: false,
                transition: None,
            },
        }
    }

    /// Refresh a session's liveness (`last_seen`, un-stale, `cwd`) WITHOUT
    /// changing its traffic-light state. This is the heartbeat used by
    /// `PostToolUse` and by every subagent event — the latter must keep the
    /// session alive and feed the subagent count without recoloring the row. If
    /// the session is unknown (we never saw it start), fall back to creating it
    /// as Working via `transition_to`, which also rehydrates any remembered
    /// terminal capture.
    fn heartbeat(&mut self, ev: &HookEvent) -> ApplyOutcome {
        if let Some(s) = self.sessions.get_mut(&ev.session_id) {
            s.last_seen = Instant::now();
            s.stale = false;
            if !ev.cwd.is_empty() {
                s.cwd = ev.cwd.clone();
            }
            ApplyOutcome {
                changed: true,
                transition: None,
            }
        } else {
            self.transition_to(ev, State::Working)
        }
    }

    /// Upsert the session and move it to `state`, refreshing timers. Returns a
    /// transition only when the state actually changed (or the session is new),
    /// so callers/notifications never fire on a same-state repeat.
    fn transition_to(&mut self, ev: &HookEvent, state: State) -> ApplyOutcome {
        let now = Instant::now();
        // A terminal handle remembered across a restart (seeded at startup). It's
        // attached only when this session's row is (re)created or is still
        // missing a handle — so a Beacon restart keeps click-to-focus for
        // already-running sessions, which never re-fire `SessionStart`.
        let remembered = self.pending_captures.get(&ev.session_id).cloned();
        // `from`: Some(prev) on a real change, None on a same-state repeat.
        let (from, cwd, terminal_pid) = match self.sessions.entry(ev.session_id.clone()) {
            std::collections::hash_map::Entry::Occupied(mut o) => {
                let s = o.get_mut();
                let prev = s.state;
                let changed_state = prev != state;
                if changed_state {
                    s.state = state;
                    s.state_since = now;
                }
                s.last_seen = now;
                s.stale = false;
                if !ev.cwd.is_empty() {
                    s.cwd = ev.cwd.clone();
                }
                // Backfill a remembered handle if this row never resolved one.
                if s.terminal_pid.is_none() {
                    if let Some(cap) = &remembered {
                        s.terminal_pid = cap.pid;
                        s.terminal_app = cap.app.clone();
                        s.terminal_tty = cap.tty.clone();
                    }
                }
                (
                    if changed_state { Some(Some(prev)) } else { None },
                    s.cwd.clone(),
                    s.terminal_pid,
                )
            }
            std::collections::hash_map::Entry::Vacant(v) => {
                let cwd = ev.cwd.clone();
                let cap = remembered.unwrap_or_default();
                let pid = cap.pid;
                v.insert(Session {
                    cwd: cwd.clone(),
                    state,
                    last_seen: now,
                    state_since: now,
                    stale: false,
                    subagent_count: 0,
                    sub_since: None,
                    terminal_pid: cap.pid,
                    terminal_app: cap.app,
                    terminal_tty: cap.tty,
                });
                (Some(None), cwd, pid)
            }
        };

        let transition = from.map(|prev| Transition {
            session_id: ev.session_id.clone(),
            label: label_for(&cwd),
            from: prev,
            to: state,
            terminal_pid,
        });
        ApplyOutcome {
            changed: true,
            transition,
        }
    }

    /// Clear a session's live subagent count + elapsed anchor. Used on
    /// `SessionStart` so a (re)started session never carries a stale sub-line.
    /// A no-op if the session isn't tracked yet.
    fn reset_subagents(&mut self, id: &str) {
        if let Some(s) = self.sessions.get_mut(id) {
            s.subagent_count = 0;
            s.sub_since = None;
        }
    }

    /// Seed a remembered terminal handle at startup (from the persisted store).
    /// It will be attached to the session the moment a real hook event recreates
    /// its row — never on its own, so this can't conjure a phantom session.
    /// Ignored if it carries no pid (nothing to focus).
    pub fn seed_capture(&mut self, session_id: String, cap: CapturedTerminal) {
        if cap.pid.is_some() {
            self.pending_captures.insert(session_id, cap);
        }
    }

    /// The captured terminal pid for a session, if Beacon resolved one. Used by
    /// the click-to-focus command to know which window to raise.
    pub fn terminal_pid(&self, id: &str) -> Option<i32> {
        self.sessions.get(id).and_then(|s| s.terminal_pid)
    }

    /// The full focus target for a session: `(pid, tty, app)`. The tty + app let
    /// `focus.rs` select the exact tab on macOS terminals; the pid is the
    /// app-level fallback. `None` until Beacon captured at least a pid.
    pub fn focus_target(&self, id: &str) -> Option<(i32, Option<String>, Option<String>)> {
        self.sessions
            .get(id)
            .and_then(|s| s.terminal_pid.map(|p| (p, s.terminal_tty.clone(), s.terminal_app.clone())))
    }

    /// Update the stale timeout at runtime (settings change). Existing sessions
    /// are re-evaluated on the next sweep.
    pub fn set_stale_timeout(&mut self, timeout: Duration) {
        self.stale_timeout = timeout;
    }

    /// Update the idle-drop window at runtime (settings change). Existing
    /// sessions are re-evaluated on the next sweep.
    pub fn set_drop_timeout(&mut self, timeout: Duration) {
        self.drop_timeout = timeout;
    }

    /// Mark sessions stale past the timeout and drop them past the grace
    /// window. Reports whether anything changed and which sessions newly went
    /// stale (so the caller can optionally notify on idle).
    pub fn sweep(&mut self) -> SweepOutcome {
        let now = Instant::now();
        let mut changed = false;
        let mut went_stale = Vec::new();

        // Drop sessions silent past the whole idle-drop window. Until then a
        // stale session stays in the list (greyed) — it is not removed just for
        // crossing the stale timeout.
        let before = self.sessions.len();
        let drop_after = self.drop_timeout;
        self.sessions
            .retain(|_, s| now.duration_since(s.last_seen) < drop_after);
        if self.sessions.len() != before {
            changed = true;
        }

        // Mark the remainder stale/fresh based on the timeout.
        for (id, s) in self.sessions.iter_mut() {
            let idle = now.duration_since(s.last_seen);
            let should_be_stale = idle >= self.stale_timeout;
            if should_be_stale != s.stale {
                if should_be_stale {
                    went_stale.push((id.clone(), label_for(&s.cwd)));
                    // A session we've declared silent ("No response") must not keep
                    // asserting live subagents — the matching SubagentStop may simply
                    // never have arrived. Clear the count so a greyed row doesn't read
                    // "idle · 1 agent running".
                    s.subagent_count = 0;
                    s.sub_since = None;
                }
                s.stale = should_be_stale;
                changed = true;
            }
        }
        SweepOutcome {
            changed,
            went_stale,
        }
    }

    /// Compute the tray rollup. Stale sessions are excluded; if none remain
    /// live the rollup is Grey. Priority: Red > Orange > Green.
    pub fn rollup(&self) -> Rollup {
        let mut any_working = false;
        let mut any_ready = false;
        for s in self.sessions.values() {
            if s.stale {
                continue;
            }
            match s.state {
                State::NeedsYou => return Rollup::Red,
                State::Working => any_working = true,
                State::Ready => any_ready = true,
            }
        }
        if any_working {
            Rollup::Orange
        } else if any_ready {
            Rollup::Green
        } else {
            Rollup::Grey
        }
    }

    /// A serializable snapshot of all sessions, newest-active first.
    pub fn snapshot(&self) -> Vec<SessionView> {
        let now = Instant::now();
        let mut views: Vec<SessionView> = self
            .sessions
            .iter()
            .map(|(id, s)| SessionView {
                session_id: id.clone(),
                label: label_for(&s.cwd),
                state: s.state,
                stale: s.stale,
                seconds_in_state: now.duration_since(s.state_since).as_secs(),
                subagent_count: s.subagent_count,
                subagent_seconds: s
                    .sub_since
                    .map(|t| now.duration_since(t).as_secs())
                    .unwrap_or(0),
                can_focus: s.terminal_pid.is_some(),
            })
            .collect();
        // Stable, useful ordering: live before stale, then by label.
        views.sort_by(|a, b| {
            a.stale
                .cmp(&b.stale)
                .then_with(|| a.label.cmp(&b.label))
        });
        views
    }
}

/// Build a human label from a working directory: the git **repo root**'s
/// basename plus its branch, if the cwd is inside a repo. We walk up from `cwd`
/// to find the `.git` directory so a subfolder cwd (e.g. `.../proj/src-tauri`)
/// still shows the project name and branch rather than the subfolder. Falls
/// back to `basename(cwd)` when no repo is found. No subprocess.
pub fn label_for(cwd: &str) -> String {
    if cwd.is_empty() {
        return "session".to_string();
    }
    let path = Path::new(cwd);

    if let Some(root) = find_git_root(path) {
        let base = root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("session")
            .to_string();
        return match read_git_branch(&root) {
            Some(branch) => format!("{base} ({branch})"),
            None => base,
        };
    }

    path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(cwd)
        .to_string()
}

/// Walk up from `start` to the first ancestor containing a `.git` entry (a
/// directory for a normal clone, a file for a worktree/submodule). Returns that
/// ancestor — the repo root. None if no ancestor is a git repo.
fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        if d.join(".git").exists() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

/// Resolve the current branch by reading `<root>/.git/HEAD`. Returns None if
/// HEAD is detached, unreadable, or the repo uses a `.git` file (worktree).
fn read_git_branch(root: &Path) -> Option<String> {
    let head = std::fs::read_to_string(root.join(".git").join("HEAD")).ok()?;
    let head = head.trim();
    // Typical content: "ref: refs/heads/main"
    head.strip_prefix("ref: refs/heads/")
        .map(|b| b.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(name: &str, sid: &str) -> HookEvent {
        HookEvent {
            hook_event_name: name.to_string(),
            session_id: sid.to_string(),
            cwd: "/tmp/proj".to_string(),
            ..Default::default()
        }
    }

    /// A subagent-emitted event: same `session_id` as the parent, but carrying an
    /// `agent_id` (and `agent_type`) the way real Claude Code subagent hooks do.
    fn sub_ev(name: &str, sid: &str) -> HookEvent {
        HookEvent {
            agent_id: Some("sub-1".to_string()),
            agent_type: Some("Explore".to_string()),
            ..ev(name, sid)
        }
    }

    fn notif(sid: &str, ty: &str) -> HookEvent {
        HookEvent {
            hook_event_name: "Notification".to_string(),
            session_id: sid.to_string(),
            cwd: "/tmp/proj".to_string(),
            notification_type: Some(ty.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn lifecycle_transitions() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("SessionStart", "a"));
        assert_eq!(e.rollup(), Rollup::Green);
        e.apply(&ev("UserPromptSubmit", "a"));
        assert_eq!(e.rollup(), Rollup::Orange);
        e.apply(&notif("a", "permission_prompt"));
        assert_eq!(e.rollup(), Rollup::Red);
        e.apply(&ev("Stop", "a"));
        assert_eq!(e.rollup(), Rollup::Green);
        e.apply(&ev("SessionEnd", "a"));
        assert_eq!(e.rollup(), Rollup::Grey);
    }

    #[test]
    fn rollup_priority_across_sessions() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("UserPromptSubmit", "a")); // working
        e.apply(&notif("b", "permission_prompt")); // needs you
        // One Working + one Needs-you → Red.
        assert_eq!(e.rollup(), Rollup::Red);
        e.apply(&ev("Stop", "b")); // b now ready
        assert_eq!(e.rollup(), Rollup::Orange); // a still working
    }

    #[test]
    fn ignored_notifications_do_not_change_state() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("UserPromptSubmit", "a"));
        assert_eq!(e.rollup(), Rollup::Orange);
        e.apply(&notif("a", "auth_success"));
        assert_eq!(e.rollup(), Rollup::Orange);
    }

    #[test]
    fn transition_reported_once_then_suppressed() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        // New session entering Working: transition from None.
        let out = e.apply(&ev("UserPromptSubmit", "a"));
        let t = out.transition.expect("first transition");
        assert_eq!(t.from, None);
        assert_eq!(t.to, State::Working);

        // First permission prompt: Working → NeedsYou (one transition).
        let out = e.apply(&notif("a", "permission_prompt"));
        let t = out.transition.expect("transition into needs-you");
        assert_eq!(t.from, Some(State::Working));
        assert_eq!(t.to, State::NeedsYou);

        // A second permission prompt while already NeedsYou: NO transition.
        let out = e.apply(&notif("a", "permission_prompt"));
        assert!(out.transition.is_none(), "repeat must not re-notify");
    }

    #[test]
    fn idle_prompt_does_not_turn_red() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        // A finished turn is Ready/green.
        e.apply(&ev("SessionStart", "a"));
        assert_eq!(e.rollup(), Rollup::Green);

        // An idle_prompt must NOT flip it to red, and must not be a transition.
        let out = e.apply(&notif("a", "idle_prompt"));
        assert!(out.transition.is_none(), "idle_prompt should not transition");
        assert_eq!(e.rollup(), Rollup::Green, "idle session stays green");

        // A genuine permission prompt still turns it red.
        e.apply(&notif("a", "permission_prompt"));
        assert_eq!(e.rollup(), Rollup::Red);
        // And a later idle_prompt doesn't clear the real red either.
        e.apply(&notif("a", "idle_prompt"));
        assert_eq!(e.rollup(), Rollup::Red, "idle must not clear a pending red");
    }

    #[test]
    fn heartbeat_keeps_state() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("UserPromptSubmit", "a"));
        e.apply(&ev("PostToolUse", "a"));
        assert_eq!(e.rollup(), Rollup::Orange);
    }

    #[test]
    fn compaction_shows_working_then_ready() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        // A finished turn is Ready/green.
        e.apply(&ev("SessionStart", "a"));
        assert_eq!(e.rollup(), Rollup::Green);
        // `/compact` fires PreCompact (never UserPromptSubmit) → Working/orange.
        e.apply(&ev("PreCompact", "a"));
        assert_eq!(e.rollup(), Rollup::Orange);
        // PostCompact ends compaction → back to Ready/green.
        e.apply(&ev("PostCompact", "a"));
        assert_eq!(e.rollup(), Rollup::Green);
    }

    #[test]
    fn pretooluse_starts_working() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        // A session first seen via a tool call is Working, even if we never
        // observed its UserPromptSubmit.
        let out = e.apply(&ev("PreToolUse", "a"));
        assert_eq!(e.rollup(), Rollup::Orange);
        let t = out.transition.expect("new session transitions");
        assert_eq!(t.from, None);
        assert_eq!(t.to, State::Working);
    }

    #[test]
    fn stale_sweep_excludes_then_drops() {
        // Zero timeout so everything is immediately stale; tiny grace.
        let mut e = Engine::new(Duration::from_millis(0), Duration::from_secs(3600));
        e.apply(&ev("SessionStart", "a"));
        assert_eq!(e.rollup(), Rollup::Green);
        let out = e.sweep();
        assert!(out.changed);
        assert_eq!(out.went_stale.len(), 1);
        // Stale → excluded from rollup → Grey, but still present.
        assert_eq!(e.rollup(), Rollup::Grey);
        assert_eq!(e.snapshot().len(), 1);

        // Now make drop happen immediately too.
        let mut e2 = Engine::new(Duration::from_millis(0), Duration::from_millis(0));
        e2.apply(&ev("SessionStart", "a"));
        e2.sweep();
        assert_eq!(e2.snapshot().len(), 0);
    }

    /// Live subagent count for a session id in the current snapshot.
    fn sub_count(e: &Engine, sid: &str) -> u32 {
        e.snapshot()
            .into_iter()
            .find(|v| v.session_id == sid)
            .map(|v| v.subagent_count)
            .unwrap_or(0)
    }

    fn state_of(e: &Engine, sid: &str) -> State {
        e.snapshot()
            .into_iter()
            .find(|v| v.session_id == sid)
            .map(|v| v.state)
            .expect("session present")
    }

    #[test]
    fn subagent_count_rises_and_falls() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("UserPromptSubmit", "a"));
        assert_eq!(sub_count(&e, "a"), 0);
        e.apply(&ev("SubagentStart", "a"));
        e.apply(&ev("SubagentStart", "a"));
        assert_eq!(sub_count(&e, "a"), 2);
        // Still actively working with subagents out.
        assert_eq!(e.rollup(), Rollup::Orange);
        e.apply(&ev("SubagentStop", "a"));
        assert_eq!(sub_count(&e, "a"), 1);
        e.apply(&ev("SubagentStop", "a"));
        assert_eq!(sub_count(&e, "a"), 0);
    }

    #[test]
    fn subagent_count_clamps_at_zero() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("SubagentStart", "a"));
        // Two stops for one start must not underflow.
        e.apply(&ev("SubagentStop", "a"));
        e.apply(&ev("SubagentStop", "a"));
        assert_eq!(sub_count(&e, "a"), 0);
        // And a fresh start still works afterwards.
        e.apply(&ev("SubagentStart", "a"));
        assert_eq!(sub_count(&e, "a"), 1);
    }

    #[test]
    fn subagent_counts_are_per_session() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        // Interleaved starts/stops across two concurrent sessions.
        e.apply(&ev("SubagentStart", "a"));
        e.apply(&ev("SubagentStart", "b"));
        e.apply(&ev("SubagentStart", "a"));
        e.apply(&ev("SubagentStop", "b"));
        assert_eq!(sub_count(&e, "a"), 2);
        assert_eq!(sub_count(&e, "b"), 0);
    }

    #[test]
    fn sub_since_anchors_only_while_busy() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("SubagentStart", "a"));
        // Busy → a real elapsed anchor (seconds is small but the field exists).
        let v = e.snapshot().into_iter().find(|v| v.session_id == "a").unwrap();
        assert_eq!(v.subagent_count, 1);
        // Idle → seconds reported as 0.
        e.apply(&ev("SubagentStop", "a"));
        let v = e.snapshot().into_iter().find(|v| v.session_id == "a").unwrap();
        assert_eq!(v.subagent_count, 0);
        assert_eq!(v.subagent_seconds, 0);
    }

    #[test]
    fn beacon_terminal_records_pid_and_survives_state_changes() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        // Capture arrives (possibly before SessionStart) and records the pid.
        let mut cap = ev("BeaconTerminal", "a");
        cap.terminal_pid = Some(4242);
        cap.terminal_app = Some("iTerm2".to_string());
        e.apply(&cap);
        assert_eq!(e.terminal_pid("a"), Some(4242));
        // can_focus is exposed in the snapshot.
        assert!(e.snapshot().iter().find(|v| v.session_id == "a").unwrap().can_focus);

        // State changes don't drop the captured terminal, and the transition
        // carries the pid for focus-aware notifications.
        let out = e.apply(&notif("a", "permission_prompt"));
        assert_eq!(e.terminal_pid("a"), Some(4242), "pid survives state change");
        assert_eq!(out.transition.unwrap().terminal_pid, Some(4242));
    }

    #[test]
    fn no_terminal_means_cannot_focus() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("SessionStart", "a"));
        assert_eq!(e.terminal_pid("a"), None);
        assert!(!e.snapshot()[0].can_focus, "no pid ⇒ no focus affordance");
    }

    #[test]
    fn seeded_capture_rehydrates_on_next_event_but_never_conjures_a_row() {
        // Simulates a Beacon restart: a handle was persisted last run and seeded
        // back in, but the session hasn't emitted anything yet.
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.seed_capture(
            "a".to_string(),
            CapturedTerminal {
                pid: Some(7777),
                app: Some("Terminal".to_string()),
                tty: Some("/dev/ttys003".to_string()),
            },
        );
        // A seed alone must NOT create a session row (no phantom rows).
        assert!(e.snapshot().is_empty(), "seeding alone creates no session");
        assert_eq!(e.terminal_pid("a"), None);

        // The first real event recreates the row AND picks up the handle, so
        // click-to-focus is back without a fresh SessionStart.
        e.apply(&ev("PostToolUse", "a"));
        assert_eq!(e.terminal_pid("a"), Some(7777), "handle rehydrated on first event");
        let v = e.snapshot().into_iter().find(|v| v.session_id == "a").unwrap();
        assert!(v.can_focus, "rehydrated handle restores the focus affordance");
        assert_eq!(e.focus_target("a"), Some((7777, Some("/dev/ttys003".to_string()), Some("Terminal".to_string()))));
    }

    #[test]
    fn session_end_forgets_seeded_capture() {
        // A seeded handle must not outlive an explicit SessionEnd, so a later
        // same-id session can't inherit a dead terminal's pid.
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.seed_capture(
            "a".to_string(),
            CapturedTerminal { pid: Some(7777), app: None, tty: None },
        );
        e.apply(&ev("SessionEnd", "a"));
        // Recreating the row now must NOT resurrect the forgotten handle.
        e.apply(&ev("PostToolUse", "a"));
        assert_eq!(e.terminal_pid("a"), None, "SessionEnd dropped the seed");
    }

    // --- Subagent events must not overwrite the parent's traffic-light state ---

    #[test]
    fn subagent_tool_calls_do_not_clear_needs_you() {
        // The reported bug: blocked on a permission while subagents run, a
        // subagent's tool calls must NOT flip the row off red.
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&notif("a", "permission_prompt"));
        assert_eq!(e.rollup(), Rollup::Red);
        e.apply(&sub_ev("PreToolUse", "a"));
        e.apply(&sub_ev("PostToolUse", "a"));
        assert_eq!(e.rollup(), Rollup::Red, "subagent activity kept NeedsYou");
        assert_eq!(state_of(&e, "a"), State::NeedsYou);
    }

    #[test]
    fn subagent_stop_does_not_clear_needs_you() {
        // SubagentStop used to call transition_to(Ready) — it must not, or the
        // last subagent finishing would turn a still-blocked parent green.
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&sub_ev("SubagentStart", "a"));
        e.apply(&notif("a", "permission_prompt"));
        assert_eq!(e.rollup(), Rollup::Red);
        e.apply(&sub_ev("SubagentStop", "a"));
        assert_eq!(state_of(&e, "a"), State::NeedsYou, "subagent stop left red intact");
        assert_eq!(sub_count(&e, "a"), 0, "but the count still decremented");
    }

    #[test]
    fn main_agent_pretooluse_clears_needs_you() {
        // The legitimate path off red: the user approves, the MAIN agent's tool
        // runs (agent_id absent) → Working.
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&notif("a", "permission_prompt"));
        assert_eq!(e.rollup(), Rollup::Red);
        e.apply(&ev("PreToolUse", "a")); // main agent, no agent_id
        assert_eq!(state_of(&e, "a"), State::Working, "user-approved tool flips to Working");
    }

    #[test]
    fn subagent_permission_prompt_still_escalates() {
        // A block is a block: a subagent hitting a permission gate must set
        // NeedsYou even though it's a subagent event.
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("UserPromptSubmit", "a"));
        assert_eq!(e.rollup(), Rollup::Orange);
        let mut block = notif("a", "permission_prompt");
        block.agent_id = Some("sub-1".to_string());
        e.apply(&block);
        assert_eq!(state_of(&e, "a"), State::NeedsYou, "subagent block still needs the user");
    }

    #[test]
    fn subagent_count_independent_of_main_state() {
        // State is pinned by main-agent events; the count moves only with
        // subagent start/stop — the two never interfere.
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("UserPromptSubmit", "a")); // main → Working
        e.apply(&sub_ev("SubagentStart", "a"));
        e.apply(&sub_ev("SubagentStart", "a"));
        assert_eq!(state_of(&e, "a"), State::Working);
        assert_eq!(sub_count(&e, "a"), 2);
        e.apply(&sub_ev("SubagentStop", "a")); // a subagent ends mid-turn...
        assert_eq!(state_of(&e, "a"), State::Working, "...but the parent stays Working");
        assert_eq!(sub_count(&e, "a"), 1);
    }

    #[test]
    fn stale_sweep_clears_subagent_count() {
        // A greyed "No response" row must not keep claiming running agents.
        let mut e = Engine::new(Duration::from_millis(0), Duration::from_secs(3600));
        e.apply(&ev("UserPromptSubmit", "a"));
        e.apply(&sub_ev("SubagentStart", "a"));
        assert_eq!(sub_count(&e, "a"), 1);
        e.sweep(); // stale_timeout is 0 → goes stale immediately
        assert!(e.snapshot()[0].stale);
        assert_eq!(sub_count(&e, "a"), 0, "stale row drops its agent count");
    }

    #[test]
    fn session_start_resets_subagents() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("SubagentStart", "a"));
        e.apply(&ev("SubagentStart", "a"));
        assert_eq!(sub_count(&e, "a"), 2);
        // A (re)start of the same id clears the count.
        e.apply(&ev("SessionStart", "a"));
        assert_eq!(sub_count(&e, "a"), 0);
    }

    #[test]
    fn idle_session_persists_until_drop_window() {
        // Stale immediately, but a long drop window: an idle session must stay
        // in the list (greyed) across repeated sweeps rather than blink out.
        let mut e = Engine::new(Duration::from_millis(0), Duration::from_secs(3600));
        e.apply(&ev("SessionStart", "a"));
        for _ in 0..3 {
            e.sweep();
        }
        assert_eq!(e.snapshot().len(), 1, "stale session stays visible");
        assert!(e.snapshot()[0].stale, "and is marked stale (grey)");
        assert_eq!(e.rollup(), Rollup::Grey);
    }
}
