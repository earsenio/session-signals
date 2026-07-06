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
    /// Short human descriptor of what this session is about — Claude Code's own
    /// generated session title (the `ai-title` in the transcript), falling back
    /// to the first human prompt. Derived locally from `transcript_path` by
    /// `descriptor::extract` and cached here so `snapshot` never does file I/O.
    /// `None` until the transcript yields one (e.g. a brand-new session).
    descriptor: Option<String>,
    /// When we last *attempted* to (re)derive `descriptor`. Debounces the
    /// transcript read so we don't re-scan the file on every hook event.
    descriptor_checked_at: Option<Instant>,
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
    /// Absolute path to this session's transcript JSONL. Present on every real
    /// Claude Code hook event (not on the synthetic `BeaconTerminal`). The source
    /// for the session descriptor — see `descriptor::extract`.
    #[serde(default)]
    pub transcript_path: Option<String>,
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
    /// The label's folder part alone (no branch) — what notifications title
    /// with, shipped separately so nothing re-parses the combined label.
    pub folder: String,
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
    /// Combined one-line label (`folder (branch)` or `folder`) — used for
    /// sorting and plain-text surfaces (tray tooltips, notifications).
    pub label: String,
    /// The label's structured parts, so the widget's two-tone row never has to
    /// re-parse `label` (a folder literally named `foo (bar)` would misparse).
    pub folder: String,
    pub branch: Option<String>,
    /// True when the session's cwd is a linked git worktree. The UI shows a
    /// subtle marker so a worktree session is distinguishable from a checkout of
    /// the same repo. Display-only.
    pub worktree: bool,
    pub state: State,
    pub stale: bool,
    /// Seconds the session has been in its current state.
    pub seconds_in_state: u64,
    /// Live subagents running under this session (`SubagentStart` − `SubagentStop`).
    pub subagent_count: u32,
    /// Seconds since the subagent count rose from 0 (0 when none are running).
    pub subagent_seconds: u64,
    /// Whether Session Signals resolved the owning terminal window — gates the widget's
    /// click-to-focus affordance (no handle ⇒ no focus button).
    pub can_focus: bool,
    /// Short human descriptor of what the session is about (Claude Code's own
    /// session title, else the first prompt). `None` until derivable. Display-only.
    pub descriptor: Option<String>,
}

/// A terminal handle remembered across a Session Signals restart. Capture lives only in
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

/// How long after `SessionEnd` a *heartbeat* for that session id is ignored
/// instead of resurrecting the row. Subagent stragglers (`PostToolUse`,
/// `PostToolBatch`, `SubagentStop`) can arrive moments after the main agent
/// ends the session; without this they'd recreate it as Working until the
/// stale sweep. Real restarts are unaffected: state-setting events
/// (`SessionStart`, `UserPromptSubmit`, …) clear the tombstone.
const END_TOMBSTONE: Duration = Duration::from_secs(30);

pub struct Engine {
    sessions: HashMap<String, Session>,
    /// Remembered terminal handles keyed by `session_id`, rehydrated at startup.
    /// Consulted when a session is (re)created so a restart keeps click-to-focus;
    /// never iterated to build the session list (it cannot create rows).
    pending_captures: HashMap<String, CapturedTerminal>,
    /// Recently-ended session ids (`SessionEnd` time), so straggler heartbeats
    /// can't resurrect them. Entries expire after [`END_TOMBSTONE`]; pruned on
    /// sweep.
    recent_ends: HashMap<String, Instant>,
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
            recent_ends: HashMap::new(),
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
                        descriptor: None,
                        descriptor_checked_at: None,
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
                // Tombstone the id briefly so straggler heartbeats (a subagent's
                // late PostToolUse) can't resurrect the row — see `heartbeat`.
                self.recent_ends
                    .insert(ev.session_id.clone(), Instant::now());
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
    /// terminal capture — unless the id was just `SessionEnd`ed, in which case
    /// the event is a straggler and is dropped rather than resurrecting the row.
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
        } else if self
            .recent_ends
            .get(&ev.session_id)
            .is_some_and(|t| t.elapsed() < END_TOMBSTONE)
        {
            ApplyOutcome {
                changed: false,
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
        // A state-setting event is real activity — a genuine restart/resume of
        // this id, never a straggler — so any end-tombstone is void.
        self.recent_ends.remove(&ev.session_id);
        // A terminal handle remembered across a restart (seeded at startup). It's
        // attached only when this session's row is (re)created or is still
        // missing a handle — so a Session Signals restart keeps click-to-focus for
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
                    if changed_state {
                        Some(Some(prev))
                    } else {
                        None
                    },
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
                    descriptor: None,
                    descriptor_checked_at: None,
                });
                (Some(None), cwd, pid)
            }
        };

        let transition = from.map(|prev| {
            let (folder, branch) = label_parts(&cwd);
            Transition {
                session_id: ev.session_id.clone(),
                label: combine_label(folder.clone(), branch.as_deref()),
                folder,
                from: prev,
                to: state,
                terminal_pid,
            }
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

    /// The captured terminal pid for a session, if Session Signals resolved one. Used by
    /// the click-to-focus command to know which window to raise.
    pub fn terminal_pid(&self, id: &str) -> Option<i32> {
        self.sessions.get(id).and_then(|s| s.terminal_pid)
    }

    /// The full focus target for a session: `(pid, tty, app)`. The tty + app let
    /// `focus.rs` select the exact tab on macOS terminals; the pid is the
    /// app-level fallback. `None` until Session Signals captured at least a pid.
    pub fn focus_target(&self, id: &str) -> Option<(i32, Option<String>, Option<String>)> {
        self.sessions.get(id).and_then(|s| {
            s.terminal_pid
                .map(|p| (p, s.terminal_tty.clone(), s.terminal_app.clone()))
        })
    }

    /// Whether the session's descriptor is worth (re)deriving from its transcript
    /// now. The caller does the (off-lock) file read only when this says so,
    /// keeping transcript I/O off the hot path. While we still have *no*
    /// descriptor we retry on the shorter `retry` cadence (so the title shows up
    /// soon after Claude Code writes it); once one is resolved we only re-check on
    /// the longer `refresh` cadence (it rarely changes). `None` if the session is
    /// gone or has never been checked (→ due immediately).
    pub fn descriptor_due(&self, id: &str, retry: Duration, refresh: Duration) -> bool {
        match self.sessions.get(id) {
            None => false,
            Some(s) => {
                let interval = if s.descriptor.is_none() {
                    retry
                } else {
                    refresh
                };
                match s.descriptor_checked_at {
                    None => true,
                    Some(t) => t.elapsed() >= interval,
                }
            }
        }
    }

    /// Record the result of a descriptor derivation. Always stamps the check time
    /// (so a fruitless read still debounces); updates the cached descriptor when
    /// the value actually changed. Returns true if the displayed value changed
    /// (worth a UI refresh). A no-op if the session is gone.
    pub fn set_descriptor(&mut self, id: &str, value: Option<String>) -> bool {
        match self.sessions.get_mut(id) {
            None => false,
            Some(s) => {
                s.descriptor_checked_at = Some(Instant::now());
                if value.is_some() && value != s.descriptor {
                    s.descriptor = value;
                    true
                } else {
                    false
                }
            }
        }
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

        // Expired end-tombstones are useless — prune so the map can't grow.
        self.recent_ends
            .retain(|_, t| now.duration_since(*t) < END_TOMBSTONE);

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
            .map(|(id, s)| {
                let (folder, branch, worktree) = label_parts_worktree(&s.cwd);
                SessionView {
                    session_id: id.clone(),
                    label: combine_label(folder.clone(), branch.as_deref()),
                    folder,
                    branch,
                    worktree,
                    state: s.state,
                    stale: s.stale,
                    seconds_in_state: now.duration_since(s.state_since).as_secs(),
                    subagent_count: s.subagent_count,
                    subagent_seconds: s
                        .sub_since
                        .map(|t| now.duration_since(t).as_secs())
                        .unwrap_or(0),
                    can_focus: s.terminal_pid.is_some(),
                    descriptor: s.descriptor.clone(),
                }
            })
            .collect();
        // Stable, useful ordering: live before stale, then by label.
        views.sort_by(|a, b| a.stale.cmp(&b.stale).then_with(|| a.label.cmp(&b.label)));
        views
    }
}

/// Repo facts derived from a working directory, used to build the session label
/// and the worktree marker. Pure filesystem reads — no subprocess.
struct GitInfo {
    /// Basename shown to the user. For a linked worktree this is the **main
    /// repo's** name (e.g. `cc-beacon`), never the worktree folder's name.
    base: String,
    /// Current branch, or None when HEAD is detached/unreadable.
    branch: Option<String>,
    /// True when `cwd` lives in a `git worktree` (a `.git` *file* plus a
    /// `commondir`). Submodules use a `.git` file too but have no `commondir`,
    /// so they are not flagged.
    worktree: bool,
}

/// Build a human label from a working directory, structured: the git **repo
/// root**'s basename plus its branch, if the cwd is inside a repo. We walk up
/// from `cwd` to find the `.git` entry so a subfolder cwd (e.g.
/// `.../proj/src-tauri`) still shows the project name and branch rather than
/// the subfolder. Falls back to `(basename(cwd), None)` when no repo is found.
/// No subprocess. Shipped as separate parts so renderers never re-parse the
/// combined string (a folder literally named `foo (bar)` would misparse).
pub fn label_parts(cwd: &str) -> (String, Option<String>) {
    let (folder, branch, _) = label_parts_worktree(cwd);
    (folder, branch)
}

/// Like [`label_parts`] but also reports whether the cwd is a linked git
/// worktree, so the widget can flag it without re-reading the filesystem. For a
/// worktree the folder is the **main repo's** name and the branch is the
/// worktree's own checkout (both of which a `.git` *file* would otherwise hide).
pub fn label_parts_worktree(cwd: &str) -> (String, Option<String>, bool) {
    match git_info(cwd) {
        Some(info) => (info.base, info.branch, info.worktree),
        None => (fallback_basename(cwd), None, false),
    }
}

/// `basename(cwd)` for the no-repo case; "session" for an empty cwd.
fn fallback_basename(cwd: &str) -> String {
    if cwd.is_empty() {
        return "session".to_string();
    }
    Path::new(cwd)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(cwd)
        .to_string()
}

/// The combined one-line label: `folder (branch)` or just `folder`.
pub fn label_for(cwd: &str) -> String {
    let (folder, branch) = label_parts(cwd);
    combine_label(folder, branch.as_deref())
}

fn combine_label(folder: String, branch: Option<&str>) -> String {
    match branch {
        Some(b) => format!("{folder} ({b})"),
        None => folder,
    }
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

/// Resolve repo facts for `cwd` from the filesystem, transparently handling
/// linked worktrees (where `.git` is a *file* pointing at the real git dir).
fn git_info(cwd: &str) -> Option<GitInfo> {
    if cwd.is_empty() {
        return None;
    }
    let root = find_git_root(Path::new(cwd))?;
    let dotgit = root.join(".git");

    // Normal clone: `.git` is a directory; branch lives at `.git/HEAD`.
    if dotgit.is_dir() {
        return Some(GitInfo {
            base: basename(&root)?,
            branch: read_head_branch(&dotgit),
            worktree: false,
        });
    }

    // `.git` is a *file* → linked worktree or submodule. It points at the real
    // git dir ("gitdir: <path>"), where this checkout's HEAD lives.
    let gitdir = read_gitdir_pointer(&dotgit)?;
    let branch = read_head_branch(&gitdir);

    // A linked worktree's git dir carries a `commondir` pointing back at the
    // shared repo; the repo root is that common dir's parent, giving us the main
    // repo's name. Submodules have no `commondir`, so they keep their own folder
    // name and are not flagged as worktrees.
    match worktree_repo_root(&gitdir) {
        Some(repo_root) => Some(GitInfo {
            base: basename(&repo_root).or_else(|| basename(&root))?,
            branch,
            worktree: true,
        }),
        None => Some(GitInfo {
            base: basename(&root)?,
            branch,
            worktree: false,
        }),
    }
}

fn basename(p: &Path) -> Option<String> {
    p.file_name().and_then(|s| s.to_str()).map(str::to_string)
}

/// Resolve the current branch by reading `<gitdir>/HEAD`. None if HEAD is
/// detached or unreadable.
fn read_head_branch(gitdir: &Path) -> Option<String> {
    let head = std::fs::read_to_string(gitdir.join("HEAD")).ok()?;
    // Typical content: "ref: refs/heads/main".
    head.trim()
        .strip_prefix("ref: refs/heads/")
        .map(str::to_string)
}

/// Read a `.git` *file* (worktree/submodule) and resolve the git dir it points
/// at. Content is "gitdir: <path>"; a relative path resolves against the file's
/// directory. Canonicalized so `..` segments collapse (best-effort).
fn read_gitdir_pointer(dotgit_file: &Path) -> Option<PathBuf> {
    let content = std::fs::read_to_string(dotgit_file).ok()?;
    let raw = content.trim().strip_prefix("gitdir:")?.trim();
    let p = Path::new(raw);
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        dotgit_file.parent()?.join(p)
    };
    Some(std::fs::canonicalize(&abs).unwrap_or(abs))
}

/// Given a linked worktree's git dir (`…/.git/worktrees/<name>`), resolve the
/// shared repo's working-tree root via its `commondir` file (relative to the git
/// dir). None when there is no `commondir` — i.e. not a linked worktree.
fn worktree_repo_root(gitdir: &Path) -> Option<PathBuf> {
    let content = std::fs::read_to_string(gitdir.join("commondir")).ok()?;
    let p = Path::new(content.trim());
    // `common` is the shared `.git` dir; the repo root is its parent.
    let common = if p.is_absolute() {
        p.to_path_buf()
    } else {
        gitdir.join(p)
    };
    let common = std::fs::canonicalize(&common).unwrap_or(common);
    common.parent().map(Path::to_path_buf)
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

    /// Labels ship structured (folder + branch), so a directory whose *name*
    /// looks like the combined form can never be misparsed by a renderer.
    #[test]
    fn label_parts_are_structured_not_reparsed() {
        let base = std::env::temp_dir().join(format!("beacon-label-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        // A plain directory literally named "foo (bar)" — folder only, no branch.
        let odd = base.join("foo (bar)");
        std::fs::create_dir_all(&odd).unwrap();
        let (folder, branch) = label_parts(odd.to_str().unwrap());
        assert_eq!(folder, "foo (bar)");
        assert_eq!(branch, None);

        // A git repo: folder = repo root basename, branch from .git/HEAD — even
        // from a subdirectory cwd.
        let repo = base.join("proj");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
        let sub = repo.join("src-tauri");
        std::fs::create_dir_all(&sub).unwrap();
        let (folder, branch) = label_parts(sub.to_str().unwrap());
        assert_eq!(folder, "proj");
        assert_eq!(branch.as_deref(), Some("main"));
        assert_eq!(label_for(sub.to_str().unwrap()), "proj (main)");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// A straggler heartbeat (e.g. a subagent's late `PostToolUse` /
    /// `SubagentStop`) arriving after `SessionEnd` must NOT resurrect the row
    /// — it would sit orange until the stale sweep. A real restart (any
    /// state-setting event, like `SessionStart` or `UserPromptSubmit`) clears
    /// the tombstone and recreates the session normally.
    #[test]
    fn straggler_heartbeat_after_session_end_does_not_resurrect() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("SessionStart", "a"));
        e.apply(&ev("SessionEnd", "a"));
        assert!(e.snapshot().is_empty());

        // Late subagent + main-agent heartbeats: dropped, no row.
        let out = e.apply(&sub_ev("PostToolUse", "a"));
        assert!(!out.changed, "straggler must not report a change");
        let out = e.apply(&sub_ev("SubagentStop", "a"));
        assert!(!out.changed);
        e.apply(&ev("PostToolBatch", "a"));
        assert!(e.snapshot().is_empty(), "no resurrection from heartbeats");

        // A genuine restart of the same id still works.
        e.apply(&ev("SessionStart", "a"));
        assert_eq!(e.snapshot().len(), 1);
        assert_eq!(e.rollup(), Rollup::Green);

        // And once restarted, heartbeats flow normally again.
        e.apply(&ev("UserPromptSubmit", "a"));
        e.apply(&sub_ev("PostToolUse", "a"));
        assert_eq!(e.rollup(), Rollup::Orange);
    }

    /// The tombstone also gives way to `UserPromptSubmit` directly (a resume
    /// that never re-fires SessionStart).
    #[test]
    fn prompt_after_session_end_recreates_session() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.apply(&ev("SessionStart", "a"));
        e.apply(&ev("SessionEnd", "a"));
        e.apply(&ev("UserPromptSubmit", "a"));
        assert_eq!(e.rollup(), Rollup::Orange);
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
        assert!(
            out.transition.is_none(),
            "idle_prompt should not transition"
        );
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
        let v = e
            .snapshot()
            .into_iter()
            .find(|v| v.session_id == "a")
            .unwrap();
        assert_eq!(v.subagent_count, 1);
        // Idle → seconds reported as 0.
        e.apply(&ev("SubagentStop", "a"));
        let v = e
            .snapshot()
            .into_iter()
            .find(|v| v.session_id == "a")
            .unwrap();
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
        assert!(
            e.snapshot()
                .iter()
                .find(|v| v.session_id == "a")
                .unwrap()
                .can_focus
        );

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
        // Simulates a Session Signals restart: a handle was persisted last run and seeded
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
        assert_eq!(
            e.terminal_pid("a"),
            Some(7777),
            "handle rehydrated on first event"
        );
        let v = e
            .snapshot()
            .into_iter()
            .find(|v| v.session_id == "a")
            .unwrap();
        assert!(
            v.can_focus,
            "rehydrated handle restores the focus affordance"
        );
        assert_eq!(
            e.focus_target("a"),
            Some((
                7777,
                Some("/dev/ttys003".to_string()),
                Some("Terminal".to_string())
            ))
        );
    }

    #[test]
    fn session_end_forgets_seeded_capture() {
        // A seeded handle must not outlive an explicit SessionEnd, so a later
        // same-id session can't inherit a dead terminal's pid.
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        e.seed_capture(
            "a".to_string(),
            CapturedTerminal {
                pid: Some(7777),
                app: None,
                tty: None,
            },
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
        assert_eq!(
            state_of(&e, "a"),
            State::NeedsYou,
            "subagent stop left red intact"
        );
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
        assert_eq!(
            state_of(&e, "a"),
            State::Working,
            "user-approved tool flips to Working"
        );
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
        assert_eq!(
            state_of(&e, "a"),
            State::NeedsYou,
            "subagent block still needs the user"
        );
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
        assert_eq!(
            state_of(&e, "a"),
            State::Working,
            "...but the parent stays Working"
        );
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
    fn descriptor_due_set_and_snapshot() {
        let retry = Duration::from_secs(5);
        let refresh = Duration::from_secs(45);
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        // Unknown session is never due.
        assert!(!e.descriptor_due("a", retry, refresh));
        e.apply(&ev("SessionStart", "a"));
        // Never checked → due immediately.
        assert!(e.descriptor_due("a", retry, refresh));
        // Setting a value reports a change, stamps checked-at, and surfaces in the
        // snapshot; a fresh check is no longer due.
        assert!(e.set_descriptor("a", Some("Audit the repo".to_string())));
        assert!(
            !e.descriptor_due("a", retry, refresh),
            "just checked → not due"
        );
        let v = e
            .snapshot()
            .into_iter()
            .find(|v| v.session_id == "a")
            .unwrap();
        assert_eq!(v.descriptor.as_deref(), Some("Audit the repo"));
        // Same value → no change reported.
        assert!(!e.set_descriptor("a", Some("Audit the repo".to_string())));
        // A fruitless re-derivation (None) must not clear an existing descriptor.
        assert!(!e.set_descriptor("a", None));
        let v = e
            .snapshot()
            .into_iter()
            .find(|v| v.session_id == "a")
            .unwrap();
        assert_eq!(
            v.descriptor.as_deref(),
            Some("Audit the repo"),
            "None doesn't clear"
        );
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

    // --- Added coverage: event→state derivation table, full rollup priority
    //     ordering, and stale-sweep exclusion / revival ---

    #[test]
    fn event_to_state_derivation_table() {
        // Each state-driving main-agent event, applied to a fresh session, lands
        // the row in the state documented in the CLAUDE.md hook contract.
        let cases: &[(&str, State)] = &[
            ("SessionStart", State::Ready),
            ("UserPromptSubmit", State::Working),
            ("PreToolUse", State::Working),
            ("PreCompact", State::Working),
            ("PostCompact", State::Ready),
            ("Stop", State::Ready),
        ];
        for (name, want) in cases {
            let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
            e.apply(&ev(name, "a"));
            assert_eq!(
                state_of(&e, "a"),
                *want,
                "event {name} should derive {want:?}"
            );
        }
        // Both NeedsYou notification types escalate identically; an ignored one
        // (idle_prompt) creates the row but leaves it Ready, never NeedsYou.
        for ty in ["permission_prompt", "elicitation_dialog"] {
            let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
            e.apply(&notif("a", ty));
            assert_eq!(state_of(&e, "a"), State::NeedsYou, "{ty} → NeedsYou");
        }
    }

    #[test]
    fn rollup_full_priority_ordering() {
        // Walk Grey → Green → Orange → Red, proving each higher-priority state
        // dominates regardless of how many lower-priority sessions are present,
        // then unwind back down as sessions end.
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        assert_eq!(e.rollup(), Rollup::Grey, "no sessions → Grey");

        e.apply(&ev("SessionStart", "ready")); // Ready
        assert_eq!(e.rollup(), Rollup::Green);

        e.apply(&ev("UserPromptSubmit", "working")); // + Working
        assert_eq!(e.rollup(), Rollup::Orange, "Working outranks Ready");

        e.apply(&notif("needs", "permission_prompt")); // + NeedsYou
        assert_eq!(e.rollup(), Rollup::Red, "NeedsYou outranks all");

        // Unwind: dropping the red session falls back to Orange (Working remains),
        // then to Green (only Ready remains), then to Grey (empty).
        e.apply(&ev("SessionEnd", "needs"));
        assert_eq!(e.rollup(), Rollup::Orange);
        e.apply(&ev("SessionEnd", "working"));
        assert_eq!(e.rollup(), Rollup::Green);
        e.apply(&ev("SessionEnd", "ready"));
        assert_eq!(e.rollup(), Rollup::Grey);
    }

    #[test]
    fn rollup_excludes_stale_sessions() {
        // A stale (silent) session must not color the tray: a session that would
        // be Red while live is invisible to the rollup once swept stale, so an
        // all-stale engine reads Grey rather than Red.
        let mut e = Engine::new(Duration::from_millis(0), Duration::from_secs(3600));
        e.apply(&notif("old", "permission_prompt")); // live → would be Red
        assert_eq!(e.rollup(), Rollup::Red);
        e.sweep(); // stale_timeout 0 → immediately stale
        assert!(e.snapshot()[0].stale, "swept stale");
        assert_eq!(e.rollup(), Rollup::Grey, "all-stale → Grey, not Red");
    }

    #[test]
    fn stale_session_revives_on_next_event() {
        // After a sweep greys a session, the next hook event un-stales it and
        // restores its color in the rollup (heartbeat clears `stale`).
        let mut e = Engine::new(Duration::from_millis(0), Duration::from_secs(3600));
        e.apply(&ev("UserPromptSubmit", "a"));
        e.sweep();
        assert!(e.snapshot()[0].stale, "went stale");
        assert_eq!(e.rollup(), Rollup::Grey);

        e.apply(&ev("PostToolUse", "a")); // heartbeat
        assert!(!e.snapshot()[0].stale, "event revived the row");
        assert_eq!(
            e.rollup(),
            Rollup::Orange,
            "back to its prior Working state"
        );
    }

    #[test]
    fn sweep_on_empty_engine_is_noop() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(3600));
        let out = e.sweep();
        assert!(!out.changed, "nothing to sweep");
        assert!(out.went_stale.is_empty());
        assert_eq!(e.rollup(), Rollup::Grey);
    }

    /// Build a throwaway repo + linked worktree on disk and check label
    /// resolution: a normal clone shows its own name + branch and isn't flagged;
    /// a linked worktree shows the **main repo's** name + the worktree's branch
    /// and is flagged.
    #[test]
    fn label_parts_resolves_clone_and_worktree() {
        use std::fs;
        let base = std::env::temp_dir().join(format!("beacon_wt_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);

        // Normal clone: `.git` is a directory with HEAD → ("myrepo", "main", not wt).
        let repo = base.join("myrepo");
        let gitdir = repo.join(".git");
        fs::create_dir_all(&gitdir).unwrap();
        fs::write(gitdir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        let (folder, branch, wt) = label_parts_worktree(repo.to_str().unwrap());
        assert_eq!(folder, "myrepo");
        assert_eq!(branch.as_deref(), Some("main"));
        assert!(!wt, "a normal clone is not a worktree");

        // Linked worktree: a separate working dir whose `.git` *file* points at
        // `<repo>/.git/worktrees/feat`, which carries its own HEAD + a commondir
        // (`../..`) resolving back to the shared `.git`.
        let wt_gitdir = gitdir.join("worktrees").join("feat");
        fs::create_dir_all(&wt_gitdir).unwrap();
        fs::write(wt_gitdir.join("HEAD"), "ref: refs/heads/feature-x\n").unwrap();
        fs::write(wt_gitdir.join("commondir"), "../..\n").unwrap();
        let wt_dir = base.join("scratch-worktree");
        fs::create_dir_all(&wt_dir).unwrap();
        fs::write(
            wt_dir.join(".git"),
            format!("gitdir: {}\n", wt_gitdir.display()),
        )
        .unwrap();
        let (folder, branch, wt) = label_parts_worktree(wt_dir.to_str().unwrap());
        assert_eq!(
            folder, "myrepo",
            "worktree shows main repo name, not the worktree folder name"
        );
        assert_eq!(branch.as_deref(), Some("feature-x"), "resolves the worktree's own branch");
        assert!(wt, "flagged as a worktree");

        let _ = fs::remove_dir_all(&base);
    }
}
