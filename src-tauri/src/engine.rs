//! State engine: the single source of truth for session status.
//!
//! Sessions are keyed by `session_id`. Hook events mutate per-session state
//! following the derivation rules in CLAUDE.md. The engine also computes the
//! tray rollup and sweeps stale (silent) sessions. It holds no Tauri handles —
//! `lib.rs` owns it behind a `Mutex` and reacts to changes by refreshing the
//! tray and emitting to the webview. The UI never derives state itself.

use serde::Serialize;
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
}

/// A state change for one session, reported by `apply` so the notification
/// engine can react. `from` is `None` when the session is brand new.
#[derive(Serialize, Clone, Debug)]
pub struct Transition {
    pub session_id: String,
    pub label: String,
    pub from: Option<State>,
    pub to: State,
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
}

pub struct Engine {
    sessions: HashMap<String, Session>,
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

        match ev.hook_event_name.as_str() {
            "SessionStart" => self.transition_to(ev, State::Ready),
            // Any work-start signal means the session is actively running. We
            // bracket "Working" between these and the terminal events below, so
            // activity that doesn't begin with a typed prompt — slash-command
            // expansion, a tool call, a spawned subagent, context compaction —
            // still shows up. (`/compact` fires PreCompact, never
            // UserPromptSubmit, which is why it used to stay green.)
            "UserPromptSubmit" | "UserPromptExpansion" | "PreToolUse" | "SubagentStart"
            | "PreCompact" => self.transition_to(ev, State::Working),
            "PostToolUse" | "PostToolUseFailure" | "PostToolBatch" => {
                // Heartbeat: keep current state, just refresh last_seen. If we
                // somehow never saw this session start, a tool running means
                // it's working.
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
            "Notification" => match ev.notification_type.as_deref() {
                // Only a genuine block on the user is "Needs you".
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
            // `StopFailure` is a turn ended by an API error.
            "Stop" | "StopFailure" | "SubagentStop" | "PostCompact" => {
                self.transition_to(ev, State::Ready)
            }
            "SessionEnd" => ApplyOutcome {
                changed: self.sessions.remove(&ev.session_id).is_some(),
                transition: None,
            },
            // Unknown / unhandled event: ignore.
            _ => ApplyOutcome {
                changed: false,
                transition: None,
            },
        }
    }

    /// Upsert the session and move it to `state`, refreshing timers. Returns a
    /// transition only when the state actually changed (or the session is new),
    /// so callers/notifications never fire on a same-state repeat.
    fn transition_to(&mut self, ev: &HookEvent, state: State) -> ApplyOutcome {
        let now = Instant::now();
        // `from`: Some(prev) on a real change, None on a same-state repeat.
        let (from, cwd) = match self.sessions.entry(ev.session_id.clone()) {
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
                (if changed_state { Some(Some(prev)) } else { None }, s.cwd.clone())
            }
            std::collections::hash_map::Entry::Vacant(v) => {
                let cwd = ev.cwd.clone();
                v.insert(Session {
                    cwd: cwd.clone(),
                    state,
                    last_seen: now,
                    state_since: now,
                    stale: false,
                });
                (Some(None), cwd)
            }
        };

        let transition = from.map(|prev| Transition {
            session_id: ev.session_id.clone(),
            label: label_for(&cwd),
            from: prev,
            to: state,
        });
        ApplyOutcome {
            changed: true,
            transition,
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
            notification_type: None,
        }
    }

    fn notif(sid: &str, ty: &str) -> HookEvent {
        HookEvent {
            hook_event_name: "Notification".to_string(),
            session_id: sid.to_string(),
            cwd: "/tmp/proj".to_string(),
            notification_type: Some(ty.to_string()),
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
