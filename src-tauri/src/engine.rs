//! State engine: the single source of truth for session status.
//!
//! Sessions are keyed by `session_id`. Hook events mutate per-session state
//! following the derivation rules in CLAUDE.md. The engine also computes the
//! tray rollup and sweeps stale (silent) sessions. It holds no Tauri handles —
//! `lib.rs` owns it behind a `Mutex` and reacts to changes by refreshing the
//! tray and emitting to the webview. The UI never derives state itself.

use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
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
    /// Grace period after going stale before the session is dropped entirely.
    grace: Duration,
}

impl Engine {
    pub fn new(stale_timeout: Duration, grace: Duration) -> Self {
        Engine {
            sessions: HashMap::new(),
            stale_timeout,
            grace,
        }
    }

    /// Apply a hook event. Returns true if it may have changed the rollup or
    /// session list (i.e. a refresh is worthwhile).
    pub fn apply(&mut self, ev: &HookEvent) -> bool {
        // An empty session_id is unusable as a key; ignore but don't crash.
        if ev.session_id.is_empty() && ev.hook_event_name != "SessionEnd" {
            return false;
        }

        match ev.hook_event_name.as_str() {
            "SessionStart" => self.set_state(ev, State::Ready),
            "UserPromptSubmit" => self.set_state(ev, State::Working),
            "PostToolUse" => {
                // Heartbeat: keep current state, just refresh last_seen. If we
                // somehow never saw this session start, a tool running means
                // it's working.
                if let Some(s) = self.sessions.get_mut(&ev.session_id) {
                    s.last_seen = Instant::now();
                    s.stale = false;
                    if !ev.cwd.is_empty() {
                        s.cwd = ev.cwd.clone();
                    }
                    true
                } else {
                    self.set_state(ev, State::Working)
                }
            }
            "Notification" => match ev.notification_type.as_deref() {
                Some("permission_prompt")
                | Some("elicitation_dialog")
                | Some("idle_prompt") => self.set_state(ev, State::NeedsYou),
                // auth_success, elicitation_complete, etc. → no state change.
                _ => false,
            },
            "Stop" | "SubagentStop" => self.set_state(ev, State::Ready),
            "SessionEnd" => self.sessions.remove(&ev.session_id).is_some(),
            // Unknown / unhandled event: ignore.
            _ => false,
        }
    }

    /// Upsert the session and move it to `state`, refreshing timers.
    fn set_state(&mut self, ev: &HookEvent, state: State) -> bool {
        let now = Instant::now();
        let entry = self.sessions.entry(ev.session_id.clone());
        match entry {
            std::collections::hash_map::Entry::Occupied(mut o) => {
                let s = o.get_mut();
                if s.state != state {
                    s.state = state;
                    s.state_since = now;
                }
                s.last_seen = now;
                s.stale = false;
                if !ev.cwd.is_empty() {
                    s.cwd = ev.cwd.clone();
                }
            }
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(Session {
                    cwd: ev.cwd.clone(),
                    state,
                    last_seen: now,
                    state_since: now,
                    stale: false,
                });
            }
        }
        true
    }

    /// Mark sessions stale past the timeout and drop them past the grace
    /// window. Returns true if anything changed.
    pub fn sweep(&mut self) -> bool {
        let now = Instant::now();
        let mut changed = false;

        // Drop sessions that have been silent past timeout + grace.
        let before = self.sessions.len();
        let drop_after = self.stale_timeout + self.grace;
        self.sessions
            .retain(|_, s| now.duration_since(s.last_seen) < drop_after);
        if self.sessions.len() != before {
            changed = true;
        }

        // Mark the remainder stale/fresh based on the timeout.
        for s in self.sessions.values_mut() {
            let idle = now.duration_since(s.last_seen);
            let should_be_stale = idle >= self.stale_timeout;
            if should_be_stale != s.stale {
                s.stale = should_be_stale;
                changed = true;
            }
        }
        changed
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

/// Build a human label from a working directory: `basename` plus the git branch
/// if it can be resolved by reading `<cwd>/.git/HEAD` (no subprocess).
pub fn label_for(cwd: &str) -> String {
    if cwd.is_empty() {
        return "session".to_string();
    }
    let path = Path::new(cwd);
    let base = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(cwd)
        .to_string();

    match read_git_branch(path) {
        Some(branch) => format!("{base} ({branch})"),
        None => base,
    }
}

/// Resolve the current branch by reading `.git/HEAD`. Returns None if not a git
/// repo or HEAD is detached / unreadable.
fn read_git_branch(cwd: &Path) -> Option<String> {
    let head = std::fs::read_to_string(cwd.join(".git").join("HEAD")).ok()?;
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
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(30));
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
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(30));
        e.apply(&ev("UserPromptSubmit", "a")); // working
        e.apply(&notif("b", "permission_prompt")); // needs you
        // One Working + one Needs-you → Red.
        assert_eq!(e.rollup(), Rollup::Red);
        e.apply(&ev("Stop", "b")); // b now ready
        assert_eq!(e.rollup(), Rollup::Orange); // a still working
    }

    #[test]
    fn ignored_notifications_do_not_change_state() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(30));
        e.apply(&ev("UserPromptSubmit", "a"));
        assert_eq!(e.rollup(), Rollup::Orange);
        e.apply(&notif("a", "auth_success"));
        assert_eq!(e.rollup(), Rollup::Orange);
    }

    #[test]
    fn heartbeat_keeps_state() {
        let mut e = Engine::new(Duration::from_secs(600), Duration::from_secs(30));
        e.apply(&ev("UserPromptSubmit", "a"));
        e.apply(&ev("PostToolUse", "a"));
        assert_eq!(e.rollup(), Rollup::Orange);
    }

    #[test]
    fn stale_sweep_excludes_then_drops() {
        // Zero timeout so everything is immediately stale; tiny grace.
        let mut e = Engine::new(Duration::from_millis(0), Duration::from_secs(3600));
        e.apply(&ev("SessionStart", "a"));
        assert_eq!(e.rollup(), Rollup::Green);
        let changed = e.sweep();
        assert!(changed);
        // Stale → excluded from rollup → Grey, but still present.
        assert_eq!(e.rollup(), Rollup::Grey);
        assert_eq!(e.snapshot().len(), 1);

        // Now make drop happen immediately too.
        let mut e2 = Engine::new(Duration::from_millis(0), Duration::from_millis(0));
        e2.apply(&ev("SessionStart", "a"));
        e2.sweep();
        assert_eq!(e2.snapshot().len(), 0);
    }
}
